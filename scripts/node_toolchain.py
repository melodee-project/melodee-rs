#!/usr/bin/env python3
"""Resolve and verify the exact pnpm version committed by a frontend project."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def required_version(project: Path) -> str:
    manager = json.loads((project / "package.json").read_text())["packageManager"]
    match = re.fullmatch(r"pnpm@(\d+\.\d+\.\d+)(?:\+sha\d+\.[a-f0-9]+)?", manager)
    if not match:
        raise RuntimeError("packageManager must pin an exact pnpm version")
    return match[1]


def command(project: Path, *, environment: dict[str, str] | None = None) -> list[str]:
    """Resolve within the same environment used to execute the frontend build."""
    version = required_version(project)
    search_path = os.pathsep.join(os.get_exec_path(environment))
    pnpm = shutil.which("pnpm", path=search_path)
    if pnpm:
        result = subprocess.run([pnpm, "--version"], cwd=project, env=environment, capture_output=True, text=True, check=False)
        if result.returncode == 0 and result.stdout.strip() == version:
            return [pnpm]
    corepack = shutil.which("corepack", path=search_path)
    if corepack:
        result = subprocess.run([corepack, "pnpm", "--version"], cwd=project, env=environment, capture_output=True, text=True, check=False)
        if result.returncode == 0 and result.stdout.strip() == version:
            return [corepack, "pnpm"]
    npx = shutil.which("npx", path=search_path)
    if npx:
        # npm exec installs into its versioned cache, without changing a user's
        # global package manager or relaxing the repository's packageManager pin.
        return [npx, "--yes", f"--package=pnpm@{version}", "--", "pnpm"]
    raise RuntimeError(f"Install pnpm@{version} or Node.js/npm (npx); the repository requires that exact version")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", choices=("admin-ui", "docs"), default="admin-ui")
    args = parser.parse_args()
    project = ROOT / args.project
    return subprocess.run([*command(project), "--version"], cwd=project, check=False).returncode

if __name__ == "__main__":
    raise SystemExit(main())
