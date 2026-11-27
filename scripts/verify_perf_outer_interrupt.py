#!/usr/bin/env python3
"""Verify owned outer fixture cleanup after a real SIGTERM or SIGINT.

This starts the tiny fixture and its lifecycle-only inner regression, then
checks that the reported parent database was removed. It never signals an
unverified process group or drops any database itself.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

import test_database

ROOT = Path(__file__).resolve().parents[1]
MAX_OUTPUT = 4 * 1024 * 1024


def stop_fixture(child: subprocess.Popen) -> None:
    """Signal only the exact child session we started, then reap it."""
    if child.poll() is None:
        try:
            if os.getpgid(child.pid) != child.pid or os.getsid(child.pid) != child.pid:
                raise RuntimeError("fixture topology changed; refusing group signal")
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    try:
        child.wait(timeout=45)
    except subprocess.TimeoutExpired:
        if os.getpgid(child.pid) != child.pid or os.getsid(child.pid) != child.pid:
            raise RuntimeError("fixture topology changed; refusing group kill")
        os.killpg(child.pid, signal.SIGKILL)
        child.wait(timeout=10)


def check_result(code: int, output: str, signal_name: str) -> str:
    expected = 130 if signal_name == "SIGINT" else 1
    if code != expected:
        raise RuntimeError("fixture did not return the expected interrupt status")
    records = []
    for line in output.splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if isinstance(value, dict) and value.get("signal_target") == "fixture":
            records.append(value)
    if len(records) != 1:
        raise RuntimeError("inner interrupt evidence is missing or ambiguous")
    record = records[0]
    name = record.get("parent_fixture")
    if (record.get("status") != "passed" or record.get("signal") != signal_name
            or not all(record.get(key) is True for key in ("clone_removed", "server_reaped", "source_intact"))
            or not isinstance(name, str) or not test_database.OWNED.fullmatch(name)):
        raise RuntimeError("inner cleanup verification failed")
    return name


def run(server: Path, migration: Path, signal_name: str, env: dict[str, str]) -> int:
    if os.name != "posix" or signal_name not in {"SIGTERM", "SIGINT"}:
        raise ValueError("outer signal test requires POSIX and a supported signal")
    server = server.resolve(strict=True)
    migration = migration.resolve(strict=True)
    if not server.is_file() or not migration.is_file():
        raise ValueError("server and migration executables must exist")
    command = [sys.executable, str(ROOT / "scripts/perf_representative.py"), "--artists", "2",
               "--releases", "3", "--tracks-per-release", "1", "--migration-cli", str(migration),
               "--", sys.executable, str(ROOT / "scripts/verify_perf_interrupt_cleanup.py"),
               "--server", str(server), "--migration-cli", str(migration),
               "--signal", signal_name, "--target", "fixture"]
    with tempfile.TemporaryFile() as output:
        child = subprocess.Popen(command, cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT,
                                 start_new_session=True)
        try:
            deadline = time.monotonic() + 120
            while True:
                try:
                    code = child.wait(timeout=15)
                    break
                except subprocess.TimeoutExpired:
                    if time.monotonic() >= deadline:
                        raise RuntimeError("outer interrupt test timed out")
        except BaseException:
            stop_fixture(child)
            raise
        if output.tell() > MAX_OUTPUT:
            raise ValueError("test output exceeds size bound")
        output.seek(0)
        name = check_result(code, output.read().decode("utf-8"), signal_name)
    admin = env.get("MELODEE_TEST_DATABASE_URL", test_database.DEFAULT_ADMIN)
    if test_database.sql(admin, f"SELECT count(*) FROM pg_database WHERE datname='{name}'") != "0":
        raise RuntimeError("fixture database was not removed")
    print(json.dumps({"status": "passed", "signal": signal_name, "outer_fixture_removed": True,
                      "clone_removed": True, "server_reaped": True,
                      "provenance": "mocked; lifecycle test only"}))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/melodee-server")
    parser.add_argument("--migration-cli", type=Path, default=ROOT / "target/debug/melodee-cli")
    parser.add_argument("--signal", choices=("SIGTERM", "SIGINT"), default="SIGTERM")
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
        return run(args.server, args.migration_cli, args.signal, os.environ.copy())
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"Outer interrupt cleanup check failed: {type(error).__name__}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("Outer interrupt cleanup check interrupted", file=sys.stderr)
        return 130
    finally:
        signal.signal(signal.SIGTERM, previous_term)
        signal.signal(signal.SIGINT, previous_int)


if __name__ == "__main__":
    raise SystemExit(main())
