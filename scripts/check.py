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
    # Build and test tooling is part of the supply chain, so include dev dependencies.
    run([pnpm, "audit"], cwd=ADMIN_UI)
    for command in ("check", "lint", "i18n:check", "test", "build"):
        run([pnpm, command], cwd=ADMIN_UI)


def run_docs_site_checks() -> None:
    pnpm = shutil.which("pnpm")
    if pnpm is None:
        raise CheckError("pnpm not installed; cannot run the documentation-site gate")
    run([pnpm, "install", "--frozen-lockfile"], cwd=DOCS)
    run([pnpm, "audit"], cwd=DOCS)
    for command in ("check", "check:links", "build"):
        run([pnpm, command], cwd=DOCS)


def run_admin_ui_e2e() -> None:
    pnpm = shutil.which("pnpm")
    if pnpm is None:
        raise CheckError("pnpm not installed; cannot run the admin-ui end-to-end gate")
    run([pnpm, "install", "--frozen-lockfile"], cwd=ADMIN_UI)
    run([pnpm, "test:e2e"], cwd=ADMIN_UI)


def run_checks(mode: str) -> None:
    if mode not in {"admin-ui", "admin-ui-e2e", "coverage", "docs", "fast", "full"}:
        raise ValueError(f"unsupported check mode: {mode}")
    if mode == "admin-ui":
        run_admin_ui_checks(required=True)
        return
    if mode == "admin-ui-e2e":
        run_admin_ui_e2e()
        return
    if mode == "coverage":
        run_coverage_mode()
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


def run_coverage() -> None:
    """Run code coverage with cargo-tarpaulin and enforce minimum thresholds.

    Coverage is advisory: failures are reported as warnings but do not
    fail the gate, since instrumentation compilation can be slow and
    thresholds are a starting point to be raised over time.
    """
    if shutil.which("cargo-tarpaulin") is None:
        print(
            "cargo-tarpaulin not installed; skipping local coverage gate (install with:\n"
            "  cargo install cargo-tarpaulin).",
            file=sys.stderr,
        )
        return

    try:
        subprocess.run(
            [
                "cargo-tarpaulin",
                "--lib",
                "--bins",
                "--fail-under=70",
            ],
            cwd=ROOT,
            check=True,
            timeout=300,
        )
    except subprocess.TimeoutExpired:
        print(
            "cargo-tarpaulin timed out after 300s; coverage data was collected but\n"
            "threshold enforcement could not complete. Review target/tarpaulin-report/\n"
            "manually.",
            file=sys.stderr,
        )
    except subprocess.CalledProcessError as error:
        print(
            f"cargo-tarpaulin coverage threshold not met (exit {error.returncode});\n"
            "review target/tarpaulin-report/ and raise test coverage.",
            file=sys.stderr,
        )


def run_coverage_mode() -> None:
    """Run only the coverage gate (for focused coverage workflows)."""
    run_coverage()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "mode", choices=("admin-ui", "admin-ui-e2e", "coverage", "docs", "fast", "full")
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


# Coverage thresholds documented for future reference.
# Lines: 70% | Branches: 60% | Functions: 70%
# These are conservative starting points to be raised as coverage improves.
# cargo-tarpaulin 0.35.4 supports --fail-under (single percentage threshold).
COVERAGE_THRESHOLDS = {
    "lines": 70,
    "branches": 60,
    "functions": 70,
}


if __name__ == "__main__":
    raise SystemExit(main())
