#!/usr/bin/env python3
"""Stage the search benchmark in an existing detached baseline worktree.

Only absent benchmark files and one known dev-dependency line are added.
Existing conflicting content or a revision mismatch is rejected before writes.
This does not build, run, reset, clean, or remove the supplied worktree.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
BASELINE = "a0979cde1fd2aeff5b8027d9d59336b678f1aa12"
FILES = (
    "crates/melodee-search/tests/perf_rebuild.rs",
    "scripts/perf_representative.py",
    "scripts/run_perf_search_case.py",
)
MANIFEST = "crates/melodee-search/Cargo.toml"
NEEDLE = "[dev-dependencies]\ntempfile = { workspace = true }\n"
DEPENDENCY = "tracing-subscriber = { workspace = true }\n"


def needs_file(path: Path, data: bytes) -> bool:
    if not path.exists():
        return True
    if not path.is_file() or path.read_bytes() != data:
        raise ValueError(f"refusing to overwrite different content: {path}")
    return False


def prepared_manifest(text: str) -> str:
    if text.count(NEEDLE) != 1:
        raise ValueError("unexpected baseline search manifest")
    if text.count(DEPENDENCY) == 1 and NEEDLE + DEPENDENCY in text:
        return text
    if DEPENDENCY in text:
        raise ValueError("conflicting tracing-subscriber entry")
    return text.replace(NEEDLE, NEEDLE + DEPENDENCY, 1)


def safe_worktree_path(worktree: Path, relative: str) -> Path:
    """Reject symlink traversal when writing to the supplied worktree."""
    path = worktree
    for part in Path(relative).parts:
        path = path / part
        if path.is_symlink():
            raise ValueError(f"refusing symlink in target path: {path}")
    return path


def prepare(worktree: Path) -> None:
    worktree = worktree.resolve(strict=True)
    def git(*args: str) -> str:
        return subprocess.run(["git", *args], cwd=worktree, check=True,
                              capture_output=True, text=True).stdout.strip()
    if Path(git("rev-parse", "--show-toplevel")).resolve() != worktree:
        raise ValueError("path must be the root of the baseline worktree")
    if git("rev-parse", "HEAD") != BASELINE:
        raise ValueError("baseline worktree has the wrong revision")
    source_helper = (ROOT / "scripts/test_database.py").read_bytes()
    if (worktree / "scripts/test_database.py").read_bytes() != source_helper:
        raise ValueError("baseline database ownership helper differs")
    files = [(safe_worktree_path(worktree, name), (ROOT / name).read_bytes()) for name in FILES]
    pending = [(path, data) for path, data in files if needs_file(path, data)]
    manifest = safe_worktree_path(worktree, MANIFEST)
    original = manifest.read_text()
    updated = prepared_manifest(original)
    for path, data in pending:
        path.write_bytes(data)
    if original != updated:
        manifest.write_text(updated)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worktree", required=True, type=Path)
    args = parser.parse_args()
    try:
        prepare(args.worktree)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Baseline preparation failed: {error}", file=sys.stderr)
        return 1
    print("Search benchmark staged in the verified baseline worktree.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
