#!/usr/bin/env python3
"""Build the release migration CLI with checked source and binary provenance."""
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


def build(repo: Path, output: Path) -> None:
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    if os.environ.get("CARGO_BUILD_TARGET"):
        raise ValueError("migration benchmark CLI must be built for the native host target")
    repo = repo.resolve(strict=True)
    if command_output(["git", "rev-parse", "--is-inside-work-tree"], repo) != "true":
        raise ValueError("repository is not a Git worktree")
    commit = command_output(["git", "rev-parse", "HEAD"], repo)
    dirty = bool(command_output(["git", "status", "--porcelain", "--untracked-files=all"], repo))
    before = source_fingerprints(repo)
    producer = file_digest(Path(__file__))
    helper = file_digest(Path(__file__).with_name("build_perf_server.py"))
    rustc = command_output(["rustc", "--version", "--verbose"], repo)
    cargo = command_output(["cargo", "--version"], repo)
    env = os.environ.copy()
    env["CARGO_BUILD_JOBS"] = "2"
    env["RUST_TEST_THREADS"] = "1"
    flags = {key: hashlib.sha256(env[key].encode()).hexdigest() if key in env else None for key in BUILD_ENV_KEYS}
    subprocess.run(["cargo", "build", "--release", "--locked", "-p", "melodee-cli", "--bin", "melodee-cli",
                    "--target-dir", str(repo / "target")], cwd=repo, env=env, check=True)
    binary = repo / "target/release" / ("melodee-cli.exe" if os.name == "nt" else "melodee-cli")
    if binary.is_symlink() or not binary.is_file():
        raise ValueError("migration CLI is not a regular file")
    if (before != source_fingerprints(repo) or command_output(["git", "rev-parse", "HEAD"], repo) != commit
            or producer != file_digest(Path(__file__))
            or helper != file_digest(Path(__file__).with_name("build_perf_server.py"))):
        raise RuntimeError("source or provenance helper changed during build")
    record = {"target": "migration-cli", "commit": commit, "worktree_dirty": dirty,
              "build_profile": "release", "cargo_build_jobs": 2,
              "rustc_version": rustc, "cargo_version": cargo, "build_environment_sha256": flags,
              "binary_sha256": file_digest(binary), "source_sha256": before,
              "provenance_producer_sha256": producer, "fingerprint_helper_sha256": helper,
              "limitations": ["matching before/after hashes cannot exclude transient edits",
                  "unlisted Cargo configuration, build environment and dependency binaries are not attested"]}
    with output.open("x", encoding="utf-8") as destination:
        json.dump(record, destination, indent=2, sort_keys=True)
        destination.write("\n")
    print(json.dumps({"binary_sha256": record["binary_sha256"]}))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=ROOT)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        build(args.repo, args.output)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Performance migration CLI build failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
