#!/usr/bin/env python3
"""Stage the policy benchmark in an existing original-revision worktree.

Only an absent/identical test and two known dev dependencies are added.
Rejects wrong revisions, symlinks and conflicting files before writing.
"""
from __future__ import annotations
import argparse
from pathlib import Path
import subprocess
import sys

from prepare_perf_search_baseline import BASELINE, needs_file, safe_worktree_path

ROOT = Path(__file__).resolve().parents[1]
TEST = "crates/melodee-policy/tests/perf_catalog_policy.rs"
MANIFEST = "crates/melodee-policy/Cargo.toml"
ADDITION = "[dev-dependencies]\ntokio.workspace = true\ntracing-subscriber.workspace = true\n\n"


def updated_manifest(text: str) -> str:
    if text.count("[lints]\n") != 1:
        raise ValueError("unexpected baseline policy manifest")
    if ADDITION + "[lints]\n" in text:
        return text
    if "[dev-dependencies]" in text:
        raise ValueError("conflicting baseline policy dev-dependencies")
    return text.replace("[lints]\n", ADDITION + "[lints]\n", 1)


def prepare(path: Path) -> None:
    root = path.resolve(strict=True)
    def git(*args: str) -> str:
        return subprocess.run(["git", *args], cwd=root, check=True,
                              capture_output=True, text=True).stdout.strip()
    if Path(git("rev-parse", "--show-toplevel")).resolve() != root or git("rev-parse", "HEAD") != BASELINE:
        raise ValueError("baseline must be the original revision at its worktree root")
    test = safe_worktree_path(root, TEST)
    manifest = safe_worktree_path(root, MANIFEST)
    data = (ROOT / TEST).read_bytes()
    pending = needs_file(test, data)
    original = manifest.read_text()
    updated = updated_manifest(original)
    if pending:
        test.write_bytes(data)
    if updated != original:
        manifest.write_text(updated)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worktree", required=True, type=Path)
    args = parser.parse_args()
    try:
        prepare(args.worktree)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Policy baseline preparation failed: {error}", file=sys.stderr)
        return 1
    print("Policy benchmark staged in the verified baseline worktree.")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
