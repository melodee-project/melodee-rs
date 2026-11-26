#!/usr/bin/env python3
"""Run scoped release-build scrobble and merge benchmarks on the owned fixture.

Invoke under scripts/perf_representative.py after compiling the benchmark test
targets. The fixture owns the database and removes it when the command exits.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import parse_qsl, urlsplit

ROOT = Path(__file__).resolve().parents[1]
OWNED = re.compile(r"melodee_template_[0-9a-f]{32}\Z")


def validate_fixture(env: dict[str, str]) -> None:
    name = env.get("MELODEE_PERF_OWNED_DATABASE", "")
    parsed = urlsplit(env.get("MELODEE__DATABASE__URL", ""))
    if (not OWNED.fullmatch(name) or parsed.scheme not in {"postgres", "postgresql"}
            or parsed.path != "/" + name
            or any(k.lower() in {"dbname", "database"} for k, _ in parse_qsl(parsed.query, keep_blank_values=True))):
        raise ValueError("refusing a database not named by the fixture invocation")


def run(sizes: list[int], env: dict[str, str]) -> int:
    validate_fixture(env)
    if not sizes or any(size not in {1000, 10000} for size in sizes) or len(set(sizes)) != len(sizes):
        raise ValueError("merge sizes must be distinct and selected from 1000 and 10000")
    command = ["cargo", "test", "--release", "-p", "melodee-persistence", "--test"]
    steps = [("perf_recommendation_contention", None)] + [
        ("perf_large_release_merge", size) for size in sizes
    ]
    for target, size in steps:
        child_env = env.copy()
        if size is not None:
            child_env["MELODEE_PERF_MERGE_TRACKS"] = str(size)
        result = subprocess.run(command + [target, "--", "--ignored", "--nocapture"],
                                cwd=ROOT, env=child_env, check=False)
        if result.returncode:
            return result.returncode
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--merge-size", type=int, choices=[1000, 10000], action="append",
                        help="repeat for each desired size (default: both 1K and 10K)")
    args = parser.parse_args()
    try:
        return run(args.merge_size or [1000, 10000], os.environ.copy())
    except (OSError, ValueError) as error:
        print(f"Mutation benchmark failed: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
