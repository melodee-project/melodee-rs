#!/usr/bin/env python3
"""Compare frozen scrobble test binaries against sequential owned fixture clones.

Invoke inside perf_representative.py. The fixture database is used only as a
read-only PostgreSQL template; this invocation owns and removes each clone.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid

import run_perf_mutation_benchmarks
import test_database

ROOT = Path(__file__).resolve().parents[1]


def binary(path: Path) -> Path:
    resolved = path.resolve(strict=True)
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise ValueError("benchmark binary must be an executable regular file")
    return resolved


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def run(control: Path, treatment: Path, rounds: int, env: dict[str, str],
        hybrid: Path | None = None) -> int:
    control, treatment = binary(control), binary(treatment)
    hybrid = binary(hybrid) if hybrid is not None else None
    binaries = [control, treatment] + ([hybrid] if hybrid else [])
    if len(set(binaries)) != len(binaries) or not 1 <= rounds <= 10:
        raise ValueError("distinct binaries and 1 to 10 rounds are required")
    run_perf_mutation_benchmarks.validate_fixture(env)
    source = env["MELODEE_PERF_OWNED_DATABASE"]
    admin = env.get("MELODEE_TEST_DATABASE_URL", test_database.DEFAULT_ADMIN)
    source_url = env["MELODEE__DATABASE__URL"]
    if test_database.database_url(admin, source) != source_url:
        raise ValueError("fixture and administrator database topology differ")
    if test_database.sql(source_url, "SELECT current_database()") != source:
        raise ValueError("fixture database identity mismatch")
    arms = [("control", control), ("treatment", treatment)]
    if hybrid is not None:
        arms.append(("hybrid", hybrid))
    arms += list(reversed(arms))
    hashes = {name: digest(path) for name, path in arms}
    print(json.dumps({"binary_sha256": hashes, "rounds_per_arm": rounds,
                      "arm_order": [name for name, _ in arms]}), flush=True)
    created: set[str] = set()
    try:
        for arm, executable in arms:
            name = "melodee_template_" + uuid.uuid4().hex
            if not test_database.OWNED.fullmatch(name) or name == source:
                raise ValueError("invalid clone name")
            start = time.monotonic()
            test_database.sql(admin, f'CREATE DATABASE "{name}" TEMPLATE "{source}"')
            created.add(name)
            clone_url = test_database.database_url(admin, name)
            if test_database.sql(clone_url, "SELECT current_database()") != name:
                raise RuntimeError("clone database identity mismatch")
            print(json.dumps({"arm": arm, "clone_seconds": time.monotonic() - start}), flush=True)
            child_env = env.copy()
            child_env.update({"MELODEE_PERF_OWNED_DATABASE": name,
                              "MELODEE__DATABASE__URL": clone_url,
                              "MELODEE_PERF_RECOMMENDATION_ROUNDS": str(rounds),
                              "RUST_TEST_THREADS": "1"})
            result = subprocess.run([str(executable), "--ignored", "--nocapture"], cwd=ROOT,
                                    env=child_env, check=False)
            if result.returncode:
                return result.returncode
            test_database.drop_owned(admin, name, created)
        return 0
    finally:
        failures = []
        for name in list(created):
            try:
                test_database.drop_owned(admin, name, created)
            except (RuntimeError, ValueError, OSError) as error:
                failures.append(f"{name}: {error}")
        if failures:
            raise RuntimeError("owned clone cleanup failed: " + "; ".join(failures))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--control", type=Path, required=True)
    parser.add_argument("--treatment", type=Path, required=True)
    parser.add_argument("--hybrid", type=Path, help="include a third treatment in ABCCBA order")
    parser.add_argument("--rounds", type=int, default=3)
    args = parser.parse_args()
    try:
        return run(args.control, args.treatment, args.rounds, os.environ.copy(), args.hybrid)
    except (OSError, ValueError, RuntimeError) as error:
        print(f"Scrobble comparison failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
