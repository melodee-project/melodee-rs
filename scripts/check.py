#!/usr/bin/env python3
"""Run the fast or full repository validation gate."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ADMIN_UI = ROOT / "admin-ui"
DOCS = ROOT / "docs"


class CheckError(RuntimeError):
    """A requested validation mode cannot run in the current environment."""


def run(command: list[str], *, cwd: Path = ROOT) -> None:
    subprocess.run(command, cwd=cwd, check=True)


def run_python_checks(*, include_tests: bool) -> None:
    for script in (
        "validate_repository.py",
        "extract_requirements.py",
        "audit_requirement_overrides.py",
        "validate_r4_client_acceptance.py",
        "validate_client_acceptance.py",
    ):
        command = [sys.executable, f"scripts/{script}"]
        if script == "extract_requirements.py":
            command.append("--check")
        run(command)

    if include_tests:
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


def run_admin_ui_checks(*, required: bool = False) -> None:
    pnpm = shutil.which("pnpm")
    if pnpm is None:
        message = "pnpm not installed; cannot run the admin-ui gate"
        if required:
            raise CheckError(message)
        print(f"{message}; skipping locally (CI installs it).", file=sys.stderr)
        return

    run([pnpm, "install", "--frozen-lockfile"], cwd=ADMIN_UI)
    for command in ("check", "lint", "i18n:check", "test", "build"):
        run([pnpm, command], cwd=ADMIN_UI)


def run_docs_site_checks() -> None:
    pnpm = shutil.which("pnpm")
    if pnpm is None:
        raise CheckError("pnpm not installed; cannot run the documentation-site gate")
    run([pnpm, "install", "--frozen-lockfile"], cwd=DOCS)
    for command in ("check", "check:links", "build"):
        run([pnpm, command], cwd=DOCS)


def run_admin_ui_e2e() -> None:
    pnpm = shutil.which("pnpm")
    if pnpm is None:
        raise CheckError("pnpm not installed; cannot run the admin-ui end-to-end gate")
    run([pnpm, "install", "--frozen-lockfile"], cwd=ADMIN_UI)
    run([pnpm, "test:e2e"], cwd=ADMIN_UI)


def run_checks(mode: str) -> None:
    if mode not in {"admin-ui", "admin-ui-e2e", "docs", "fast", "full"}:
        raise ValueError(f"unsupported check mode: {mode}")
    if mode == "admin-ui":
        run_admin_ui_checks(required=True)
        return
    if mode == "admin-ui-e2e":
        run_admin_ui_e2e()
        return
    if mode == "docs":
        run_docs_site_checks()
        return

    full = mode == "full"
    run_python_checks(include_tests=full)
    if full:
        run_admin_ui_checks()

    run(["cargo", "fmt", "--all", "--check"])
    run(
        [
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ]
    )
    if full:
        run(["cargo", "test", "--workspace", "--all-features"])
    else:
        run(
            [
                "cargo",
                "test",
                "--workspace",
                "--all-features",
                "--lib",
                "--bins",
            ]
        )

    if full:
        if shutil.which("cargo-deny") is None:
            print(
                "cargo-deny not installed; skipping local dependency audit (CI installs it).",
                file=sys.stderr,
            )
        else:
            run(["cargo", "deny", "check"])


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "mode", choices=("admin-ui", "admin-ui-e2e", "docs", "fast", "full")
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        run_checks(args.mode)
    except CheckError as error:
        print(str(error), file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
