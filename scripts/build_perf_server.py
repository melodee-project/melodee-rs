#!/usr/bin/env python3
"""Build a release server and record source fingerprints and binary identity.

No existing evidence file is overwritten. A changed source tree during the
build invalidates the record. Hash equality does not prove a reproducible build
or exclude a transient edit that was reverted before the second snapshot.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
STATIC_BUILD_INPUTS = (
    "compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-album-large.bin",
    "compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-artist-thumb.bin",
    "compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-playlist-thumb.bin",
    "compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-user-thumb.bin",
    "compatibility/opensubsonic/golden/media/os-getcoverart-default.bin",
)
BUILD_ENV_KEYS = (
    "CARGO_BUILD_TARGET", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER",
    "CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_PROFILE_RELEASE_LTO",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
)


def file_digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def source_fingerprints(root: Path) -> dict[str, str]:
    paths = [root / "Cargo.toml", root / "Cargo.lock", root / "rust-toolchain.toml"]
    paths.extend(root / name for name in STATIC_BUILD_INPUTS)
    for name in ("apps", "crates"):
        directory = root / name
        if not directory.is_dir():
            raise ValueError("workspace source directory is missing")
        for path in directory.rglob("*"):
            if path.is_symlink():
                raise ValueError("refusing symlinks in workspace source")
            if path.is_file() and not any(part in {"target", ".git"} for part in path.relative_to(root).parts):
                paths.append(path)
    if any(not path.is_file() or path.is_symlink() for path in paths):
        raise ValueError("workspace manifests must be regular files")
    return {path.relative_to(root).as_posix(): file_digest(path) for path in sorted(paths)}


def command_output(arguments: list[str], cwd: Path) -> str:
    return subprocess.run(arguments, cwd=cwd, capture_output=True, text=True, check=True, timeout=15).stdout.strip()


def build(repo: Path, output: Path) -> None:
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    repo = repo.resolve(strict=True)
    if command_output(["git", "rev-parse", "--is-inside-work-tree"], repo) != "true":
        raise ValueError("repository is not a Git worktree")
    commit = command_output(["git", "rev-parse", "HEAD"], repo)
    dirty_before = bool(command_output(["git", "status", "--porcelain", "--untracked-files=all"], repo))
    before = source_fingerprints(repo)
    rustc = command_output(["rustc", "--version", "--verbose"], repo)
    cargo = command_output(["cargo", "--version"], repo)
    target = repo / "target"
    env = os.environ.copy()
    env["CARGO_BUILD_JOBS"] = "2"
    env["RUST_TEST_THREADS"] = "1"
    build_environment_hashes = {
        key: hashlib.sha256(env[key].encode("utf-8")).hexdigest() if key in env else None
        for key in BUILD_ENV_KEYS
    }
    subprocess.run(["cargo", "build", "--release", "--locked", "-p", "melodee-server",
                    "--bin", "melodee-server", "--target-dir", str(target)],
                   cwd=repo, env=env, check=True)
    after = source_fingerprints(repo)
    if before != after or command_output(["git", "rev-parse", "HEAD"], repo) != commit:
        raise RuntimeError("source or commit changed during build; provenance is invalid")
    binary = target / "release" / ("melodee-server.exe" if os.name == "nt" else "melodee-server")
    if binary.is_symlink() or not binary.is_file():
        raise ValueError("built server is not a regular file")
    result = {"commit": commit, "worktree_dirty": dirty_before, "build_profile": "release",
              "cargo_build_jobs": 2, "rustc_version": rustc, "cargo_version": cargo,
              "build_environment_sha256": build_environment_hashes,
              "binary_sha256": file_digest(binary), "source_sha256": before,
              "limitations": ["matching before/after hashes cannot exclude transient source edits",
                              "only listed non-workspace embedded build inputs are fingerprinted",
                              "unlisted Cargo configuration and environment settings are not attested",
                              "dependency/toolchain binaries and host filesystem are not independently attested"]}
    with output.open("x", encoding="utf-8") as destination:
        json.dump(result, destination, indent=2, sort_keys=True)
        destination.write("\n")
    print(json.dumps({"output": str(output), "binary_sha256": result["binary_sha256"]}))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=ROOT, help="workspace to build; defaults to this checkout")
    parser.add_argument("--output", type=Path, required=True, help="new provenance JSON file")
    args = parser.parse_args()
    try:
        build(args.repo, args.output)
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Performance build failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
