#!/usr/bin/env python3
"""Exercise graceful SIGTERM or SIGINT cleanup on an owned tiny fixture.

Run only inside perf_representative.py. This is a lifecycle regression, not a
performance benchmark. It intentionally mocks server-build provenance and sends
the requested signal only to its own coordinator after its server reports ready.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import threading
from unittest import mock

import perf_db_competition
import run_db_competition_matrix as matrix
from run_perf_mutation_benchmarks import validate_fixture
import test_database

ROOT = Path(__file__).resolve().parents[1]


def run(server: Path, migration: Path, env: dict[str, str], signal_name: str = "SIGTERM",
        target: str = "coordinator") -> int:
    validate_fixture(env)
    if os.name != "posix":
        raise ValueError("signal lifecycle regression requires POSIX signal semantics")
    if signal_name not in {"SIGTERM", "SIGINT"}:
        raise ValueError("unsupported signal")
    if target not in {"coordinator", "fixture"}:
        raise ValueError("unsupported signal target")
    if target == "fixture":
        # Only the fixture that directly launched this private session can be
        # signalled; a manually invoked process cannot target an arbitrary PID.
        if (env.get("MELODEE_PERF_FIXTURE_PID") != str(os.getppid())
                or os.getsid(0) != os.getpid()):
            raise ValueError("fixture process/session identity mismatch")
    source = env["MELODEE_PERF_OWNED_DATABASE"]
    source_url = env["MELODEE__DATABASE__URL"]
    admin = env.get("MELODEE_TEST_DATABASE_URL", "")
    if not admin or test_database.database_url(admin, source) != source_url:
        raise ValueError("fixture and administrator topology differ")
    if test_database.sql(source_url, "SELECT current_database()") != source:
        raise ValueError("fixture database identity mismatch")
    server = server.resolve(strict=True)
    migration = migration.resolve(strict=True)
    if not server.is_file() or not migration.is_file():
        raise ValueError("server and migration executables must exist")
    shape = json.loads(test_database.sql(source_url, """SELECT json_build_object(
        'artists',(SELECT count(*) FROM artists),'releases',(SELECT count(*) FROM releases),
        'tracks',(SELECT count(*) FROM tracks),'work',(SELECT count(*) FROM release_work_items),
        'jobs',(SELECT count(*) FROM search_rebuild_jobs))"""))
    if (shape["artists"] > 10 or shape["releases"] > 10 or shape["tracks"] > 100
            or shape["work"] != 0 or shape["jobs"] != 0):
        raise ValueError("interrupt test requires a clean tiny fixture")
    releases = shape["releases"]
    if releases < 1 or shape["tracks"] % releases:
        raise ValueError("fixture must have evenly distributed tracks")
    clone_names: list[str] = []
    servers: list[object] = []
    timers: list[threading.Thread] = []
    finished = threading.Event()
    original_sql = test_database.sql
    original_start = perf_db_competition.start_server

    def sql(url: str, statement: str) -> str:
        result = original_sql(url, statement)
        if statement.startswith('CREATE DATABASE "'):
            name = statement.split('"')[1]
            if not test_database.OWNED.fullmatch(name) or name == source:
                raise RuntimeError("unexpected clone identity")
            clone_names.append(name)
        return result

    def start(*args, **kwargs):
        result = original_start(*args, **kwargs)
        servers.append(result[0])
        def stop_coordinator() -> None:
            if not finished.wait(0.5):
                # Revalidate the parent just before signalling to fail closed
                # if the process was reparented after the initial check.
                signal_pid = os.getpid()
                if target == "fixture":
                    if env.get("MELODEE_PERF_FIXTURE_PID") != str(os.getppid()):
                        return
                    signal_pid = os.getppid()
                os.kill(signal_pid, getattr(signal, signal_name))
        timer = threading.Thread(target=stop_coordinator, daemon=True)
        timers.append(timer)
        timer.start()
        return result

    with tempfile.TemporaryDirectory(prefix="melodee-interrupt-check-") as temporary:
        root = Path(temporary)
        record = root / "test-build.json"
        record.write_text(json.dumps({"binary_sha256": "mocked-for-lifecycle-test", "commit": "mocked"}))
        args = ["run_db_competition_matrix.py", "--mode", "idle", "--pool-size", "8",
                "--artists", str(shape["artists"]), "--releases", str(releases),
                "--tracks-per-release", str(shape["tracks"] // releases), "--albums", "1",
                "--seconds", "20", "--server", str(server), "--migration-cli", str(migration),
                "--provenance", str(record), "--output-dir", str(root / "results"),
                "--media-parent", str(ROOT), "--allow-missing-metrics"]
        try:
            with mock.patch.object(matrix, "verify_provenance"), \
                 mock.patch.object(test_database, "sql", side_effect=sql), \
                 mock.patch.object(perf_db_competition, "start_server", side_effect=start), \
                 mock.patch.object(sys, "argv", args), mock.patch.dict(os.environ, env, clear=True):
                result = matrix.main()
        finally:
            finished.set()
        for timer in timers:
            timer.join(timeout=5)
        if result == 0 or len(clone_names) != 1 or len(servers) != 1:
            raise RuntimeError("interrupt was not exercised as expected")
        if any(thread.is_alive() for thread in timers):
            raise RuntimeError("signal thread did not finish")
        if servers[0].poll() is None:
            raise RuntimeError("private server is still running")
        clone = clone_names[0]
        if original_sql(admin, f"SELECT count(*) FROM pg_database WHERE datname='{clone}'") != "0":
            raise RuntimeError("owned clone still exists")
        if original_sql(source_url, "SELECT current_database()") != source:
            raise RuntimeError("parent fixture changed identity")
        print(json.dumps({"status": "passed", "signal": signal_name, "clone_removed": True,
                          "server_reaped": True, "source_intact": True,
                          "signal_target": target,
                          "parent_fixture": source if target == "fixture" else None,
                          "fixture": shape, "provenance": "mocked; lifecycle test only"}))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/melodee-server")
    parser.add_argument("--migration-cli", type=Path, default=ROOT / "target/debug/melodee-cli")
    parser.add_argument("--signal", choices=("SIGTERM", "SIGINT"), default="SIGTERM")
    parser.add_argument("--target", choices=("coordinator", "fixture"), default="coordinator")
    args = parser.parse_args()
    try:
        return run(args.server, args.migration_cli, os.environ.copy(), args.signal, args.target)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Interrupt cleanup check failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
