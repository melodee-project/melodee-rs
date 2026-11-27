#!/usr/bin/env python3
"""Run a prebuilt policy benchmark with a censored whole-case deadline.

Must be launched by perf_representative.py and only accesses its owned database.
"""
from __future__ import annotations
import argparse
import os
from pathlib import Path
import subprocess

from run_perf_mutation_benchmarks import validate_fixture

ROOT = Path(__file__).resolve().parents[1]
BASELINE = "a0979cde1fd2aeff5b8027d9d59336b678f1aa12"


def resolve_binary(path: Path, baseline_worktree: Path | None = None) -> Path:
    root = ROOT
    if baseline_worktree is not None:
        root = baseline_worktree.resolve(strict=True)
        def git(*args: str) -> str:
            return subprocess.run(["git", *args], cwd=root, capture_output=True,
                                  text=True, check=True).stdout.strip()
        if Path(git("rev-parse", "--show-toplevel")).resolve() != root or git("rev-parse", "HEAD") != BASELINE:
            raise ValueError("baseline must be the original revision at its worktree root")
    directory = (root / "target" / "release" / "deps").resolve()
    binary = path.resolve(strict=True)
    if (binary.parent != directory or not binary.name.startswith("perf_catalog_policy-")
            or not binary.is_file() or not os.access(binary, os.X_OK)):
        raise ValueError("expected a prebuilt perf_catalog_policy test under target/release/deps")
    return binary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", required=True, type=Path)
    parser.add_argument("--baseline-worktree", type=Path,
                        help="allow a prebuilt test from the verified original revision")
    parser.add_argument("--timeout-seconds", type=int, default=600)
    args = parser.parse_args()
    if args.timeout_seconds < 1:
        parser.error("--timeout-seconds must be positive")
    try:
        env = os.environ.copy()
        validate_fixture(env)
        binary = resolve_binary(args.test_binary, args.baseline_worktree)
        try:
            return subprocess.run([str(binary), "--ignored", "--nocapture"], cwd=ROOT,
                                  env=env, timeout=args.timeout_seconds, check=False).returncode
        except subprocess.TimeoutExpired:
            print(f"policy_workload_timeout_censored seconds_at_least={args.timeout_seconds}", flush=True)
            return 124
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Policy benchmark failed: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
