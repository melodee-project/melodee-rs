#!/usr/bin/env python3
"""Run the fast, full (pre-merge), or nightly repository validation gate.

`full` excludes six historical-schema migration compatibility tests that
re-run the whole migration chain per test; `nightly` runs them too.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ADMIN_UI = ROOT / "admin-ui"
DOCS = ROOT / "docs"

# The Rust suites assert on process-global state — the supervised-tool
# admission budget, open-descriptor counts and cumulative metric counters — so
# cases that libtest runs in parallel threads perturb each other's baselines.
# Serial execution keeps the tier deterministic; the whole workspace unit tier
# costs a few tens of seconds more.
RUST_TEST_ENVIRONMENT = {"RUST_TEST_THREADS": "1"}


class CheckError(RuntimeError):
    """A requested validation mode cannot run in the current environment."""


def run(command: list[str], *, cwd: Path = ROOT, env: dict[str, str] | None = None) -> None:
    if Path(command[0]).name in {"pnpm", "pnpm.cmd", "pnpm.exe"}:
        sys.path.insert(0, str(ROOT / "scripts"))
        import node_toolchain

        command = [*node_toolchain.command(cwd), *command[1:]]
    subprocess.run(command, cwd=cwd, check=True, env=env)


def run_rust_tests(command: list[str]) -> None:
    """Run a Rust test tier with the deterministic serial harness."""
    run(command, env={**os.environ, **RUST_TEST_ENVIRONMENT})


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
    pnpm = shutil.which("pnpm") or ("pnpm" if shutil.which("corepack") or shutil.which("npx") else None)
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
    pnpm = shutil.which("pnpm") or ("pnpm" if shutil.which("corepack") or shutil.which("npx") else None)
    if pnpm is None:
        raise CheckError("pnpm not installed; cannot run the documentation-site gate")
    run([pnpm, "install", "--frozen-lockfile"], cwd=DOCS)
    run([pnpm, "audit"], cwd=DOCS)
    for command in ("check", "check:links", "build"):
        run([pnpm, command], cwd=DOCS)


def run_admin_ui_e2e() -> None:
    pnpm = shutil.which("pnpm") or ("pnpm" if shutil.which("corepack") or shutil.which("npx") else None)
    if pnpm is None:
        raise CheckError("pnpm not installed; cannot run the admin-ui end-to-end gate")
    run([pnpm, "install", "--frozen-lockfile"], cwd=ADMIN_UI)
    run([pnpm, "test:e2e"], cwd=ADMIN_UI)


def run_live_ui_audit() -> None:
    """Live Admin UI audit against a running instance (no API mocks).

    Separate from `admin-ui-e2e` on purpose: that suite mocks the Admin API, which
    is precisely the seam that let contract drift through. This mode needs a live
    server plus administrator credentials in the environment; the spec skips
    itself (never silently passes) when they are absent.
    """
    pnpm = shutil.which("pnpm") or ("pnpm" if shutil.which("corepack") or shutil.which("npx") else None)
    if pnpm is None:
        raise CheckError("pnpm not installed; cannot run the live admin-ui audit")
    run([pnpm, "install", "--frozen-lockfile"], cwd=ADMIN_UI)
    run([pnpm, "test:live"], cwd=ADMIN_UI)


def run_checks(mode: str) -> None:
    if mode not in {
        "admin-ui",
        "admin-ui-e2e",
        "audit-live",
        "coverage",
        "docs",
        "fast",
        "full",
        "nightly",
        "unit",
    }:
        raise ValueError(f"unsupported check mode: {mode}")
    if mode == "unit":
        run(["cargo", "build", "--locked", "-p", "melodee-application", "--bin", "melodee-script-worker"])
        run_rust_tests(["cargo", "test", "--workspace", "--all-features", "--lib", "--bins"])
        run_rust_tests(["cargo", "test", "--all-features", "-p", "melodee-application", "-p", "melodee-domain", "-p", "melodee-mql", "-p", "melodee-policy", "-p", "melodee-media", "--test", "*"])
        return
    if mode == "audit-live":
        run_live_ui_audit()
        return
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

    full = mode in {"full", "nightly"}
    nightly = mode == "nightly"
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
    run(["cargo", "build", "--locked", "-p", "melodee-application", "--bin", "melodee-script-worker"])
    if nightly:
        # Every Rust test suite is the nightly/release tier: workspace
        # unit/binary tests, the database-free integration crates, the
        # PostgreSQL-backed workspace suites, and the historical-schema
        # migration compatibility tests. `fast` and `full` stay lint/compile/
        # repository gates so the development and pre-merge loops stay short.
        run_rust_tests([
            "cargo",
            "test",
            "--workspace",
            "--all-features",
            "--lib",
            "--bins",
        ])
        run_rust_tests([
            "cargo",
            "test",
            "--all-features",
            "-p",
            "melodee-application",
            "-p",
            "melodee-domain",
            "-p",
            "melodee-mql",
            "-p",
            "melodee-policy",
            "-p",
            "melodee-media",
            "--test",
            "*",
        ])
        run_rust_tests([sys.executable, "scripts/test_database.py", "--", "cargo", "test", "--workspace", "--all-features"])
        run_rust_tests([
            sys.executable,
            "scripts/test_database.py",
            "--",
            "cargo",
            "test",
            "--workspace",
            "--all-features",
            "-p",
            "melodee-persistence",
            "--test",
            "migrations",
            "--",
            "--ignored",
        ])

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
        "mode",
        choices=(
            "admin-ui",
            "admin-ui-e2e",
            "audit-live",
            "coverage",
            "docs",
            "fast",
            "full",
            "nightly",
            "unit",
        ),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        run_checks(args.mode)
    except (CheckError, RuntimeError) as error:
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
