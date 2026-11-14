#!/usr/bin/env python3
"""Validate the local toolchain and run database-free bootstrap checks."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
UNIT_TEST_COMMAND = (
    "cargo",
    "test",
    "--workspace",
    "--all-features",
    "--lib",
    "--bins",
)


def run(command: list[str] | tuple[str, ...]) -> None:
    subprocess.run(command, cwd=ROOT, check=True)


def command_version(command: str) -> str:
    result = subprocess.run(
        [command, "--version"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def bootstrap() -> None:
    missing = [command for command in ("cargo", "rustc") if shutil.which(command) is None]
    if missing:
        for command in missing:
            print(f"Missing required command: {command}", file=sys.stderr)
        raise SystemExit(1)

    print(f"Rust: {command_version('rustc')}")
    print(f"Cargo: {command_version('cargo')}")
    print(f"Python: {sys.version.split()[0]}")

    run([sys.executable, "scripts/validate_repository.py"])
    run([sys.executable, "scripts/extract_requirements.py", "--check"])
    run([sys.executable, "scripts/audit_requirement_overrides.py"])
    run(
        [
            sys.executable,
            "-m",
            "unittest",
            "discover",
            "-s",
            "tests/python",
            "-p",
            "test_*.py",
            "-v",
        ]
    )

    lockfile = ROOT / "Cargo.lock"
    if not lockfile.is_file():
        print("Generating Cargo.lock. Commit it with the first dependency-resolving change.")
        run(["cargo", "generate-lockfile"])

    run(["cargo", "fmt", "--all", "--check"])
    print("Running database-free Rust unit tests (PostgreSQL integration tests excluded).")
    run(["cargo", "build", "--locked", "-p", "melodee-application", "--bin", "melodee-script-worker"])
    run(UNIT_TEST_COMMAND)
    run(["cargo", "test", "--all-features", "-p", "melodee-application", "-p", "melodee-domain", "-p", "melodee-mql", "-p", "melodee-policy", "-p", "melodee-media", "--test", "*"])
    if shutil.which("node") and (shutil.which("pnpm") or shutil.which("npx")):
        for project in ("admin-ui", "docs"):
            run([sys.executable, "scripts/node_toolchain.py", "--project", project])
    print("Bootstrap checks completed.")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    return parser.parse_args()


def main() -> int:
    parse_args()
    try:
        bootstrap()
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
