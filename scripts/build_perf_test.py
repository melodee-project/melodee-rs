#!/usr/bin/env python3
"""Build a selected release performance test and record source provenance.

Builds only the named test target and never runs it. No evidence file is
replaced. Matching before/after hashes do not exclude a transient source edit.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

from build_perf_server import BUILD_ENV_KEYS, command_output, file_digest, source_fingerprints

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "interactive": ("melodee-server", "perf_interactive_routes", False),
    "identity": ("melodee-catalog", "perf_identity_reconciliation", False),
    "admin-stats": ("melodee-server", "perf_admin_stats_ab", False),
    "search-outbox": ("melodee-search", "perf_outbox", False),
    "search-concurrent": ("melodee-search", "perf_outbox_concurrent", True),
}


def build(repo: Path, output: Path, target: str = "interactive") -> None:
    if target not in TARGETS:
        raise ValueError("unknown performance test target")
    package, name, needs_support = TARGETS[target]
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    repo = repo.resolve(strict=True)
    if command_output(["git", "rev-parse", "--is-inside-work-tree"], repo) != "true":
        raise ValueError("repository is not a Git worktree")
    commit = command_output(["git", "rev-parse", "HEAD"], repo)
    dirty = bool(command_output(["git", "status", "--porcelain", "--untracked-files=all"], repo))
    before = source_fingerprints(repo)
    producer_before = file_digest(Path(__file__))
    helper_before = file_digest(Path(__file__).with_name("build_perf_server.py"))
    rustc = command_output(["rustc", "--version", "--verbose"], repo)
    cargo = command_output(["cargo", "--version"], repo)
    env = os.environ.copy()
    env["CARGO_BUILD_JOBS"] = "2"
    env["RUST_TEST_THREADS"] = "1"
    hashes = {key: hashlib.sha256(env[key].encode()).hexdigest() if key in env else None
              for key in BUILD_ENV_KEYS}
    result = subprocess.run(["cargo", "test", "--release", "--locked", "-p", package,
                             *( ["--features", "test-support"] if needs_support else [] ),
                             "--test", name, "--no-run", "--message-format=json"],
                            cwd=repo, env=env, check=True, stdout=subprocess.PIPE, text=True)
    executables = set()
    for line in result.stdout.splitlines():
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if (item.get("reason") == "compiler-artifact"
                and item.get("target", {}).get("name") == name
                and item.get("profile", {}).get("test") and item.get("executable")):
            executables.add(Path(item["executable"]))
    if len(executables) != 1:
        raise RuntimeError("expected exactly one selected test executable")
    executable = executables.pop()
    if executable.is_symlink() or not executable.is_file():
        raise ValueError("test executable is not a regular file")
    after = source_fingerprints(repo)
    if (before != after or command_output(["git", "rev-parse", "HEAD"], repo) != commit
            or producer_before != file_digest(Path(__file__))
            or helper_before != file_digest(Path(__file__).with_name("build_perf_server.py"))):
        raise RuntimeError("source or commit changed during build")
    record = {"test_target": target, "commit": commit, "worktree_dirty": dirty, "build_profile": "release",
              "cargo_build_jobs": 2, "rustc_version": rustc, "cargo_version": cargo,
              "build_environment_sha256": hashes, "test_executable_sha256": file_digest(executable),
              "test_executable_name": executable.name, "source_sha256": before,
              "provenance_producer_sha256": producer_before,
              "fingerprint_helper_sha256": helper_before,
              "limitations": ["matching before/after hashes cannot exclude transient source edits",
                              "only listed non-workspace embedded build inputs are fingerprinted",
                              "unlisted Cargo configuration and environment settings are not attested",
                              "dependency/toolchain binaries and host filesystem are not independently attested"]}
    with output.open("x", encoding="utf-8") as destination:
        json.dump(record, destination, indent=2, sort_keys=True)
        destination.write("\n")
    print(json.dumps({"test_executable_name": executable.name, "sha256": record["test_executable_sha256"]}))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=sorted(TARGETS), default="interactive")
    parser.add_argument("--repo", type=Path, default=ROOT)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        build(args.repo, args.output, args.target)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Performance test build failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
