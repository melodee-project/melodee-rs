#!/usr/bin/env python3
"""Run local pre-commit/PR checks, stopping at the first failure.

Requires the pinned Rust toolchain, pnpm, cargo-deny, media tools, and a prepared
development/test PostgreSQL instance. Dependency installs and audits need network
access. Reuses check.py so prechecks follow the repository's canonical gates.
"""

from __future__ import annotations

import argparse
import shlex
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


ROOT = Path(__file__).resolve().parents[1]
REQUIRED_TOOLS = ("git", "cargo", "node", "pnpm", "cargo-deny", "ffmpeg", "ffprobe", "fpcalc")


@dataclass(frozen=True)
class Check:
    """One named command in the fail-fast sequence."""

    name: str
    command: tuple[str, ...]


def checks_for(*, include_e2e: bool = False) -> list[Check]:
    """Keep inexpensive diff checks before the canonical validation gates."""

    gate = (sys.executable, "-u", "scripts/check.py")
    checks = [
        Check("Working tree diff", ("git", "diff", "--check")),
        Check("Staged diff", ("git", "diff", "--cached", "--check")),
        Check("Full repository gate", (*gate, "full")),
        Check("Documentation site", (*gate, "docs")),
    ]
    if include_e2e:
        checks.append(Check("Admin UI end-to-end/visual tests", (*gate, "admin-ui-e2e")))
    return checks


def validate_environment() -> None:
    """Prevent optional-tool skips in check.py from looking like a complete pass."""

    missing = [tool for tool in REQUIRED_TOOLS if shutil.which(tool) is None]
    if missing:
        raise RuntimeError(
            "Required tools missing from PATH: " + ", ".join(missing)
            + ". Install the source-build prerequisites and cargo-deny, then retry."
        )


def run_checks(checks: Sequence[Check]) -> int:
    """Stream child output and report timings, failure, and any unrun gates."""

    results: list[tuple[str, float]] = []
    for index, check in enumerate(checks):
        print(f"\n[{index + 1}/{len(checks)}] {check.name}", flush=True)
        print(f"$ {shlex.join(check.command)}", flush=True)
        started = time.monotonic()
        try:
            result = subprocess.run(list(check.command), cwd=ROOT, shell=False, check=False)
        except OSError as error:
            print(f"FAIL: {check.name}: {error}", file=sys.stderr)
            return 2
        elapsed = time.monotonic() - started
        if result.returncode:
            print(
                f"FAIL: {check.name} (exit {result.returncode}, {elapsed:.1f}s).",
                file=sys.stderr,
            )
            remaining = [item.name for item in checks[index + 1:]]
            if remaining:
                print("Not run: " + ", ".join(remaining), file=sys.stderr)
            # subprocess uses negative codes for signals; map to shell conventions.
            return result.returncode if result.returncode > 0 else 128 - result.returncode
        results.append((check.name, elapsed))
        print(f"PASS: {check.name} ({elapsed:.1f}s)", flush=True)

    print("\nPrecheck results:", flush=True)
    for name, elapsed in results:
        print(f"  PASS  {name}: {elapsed:.1f}s", flush=True)
    print(
        f"All {len(results)} selected prechecks passed "
        f"in {sum(elapsed for _, elapsed in results):.1f}s.",
        flush=True,
    )
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--include-e2e",
        action="store_true",
        help="also run Admin UI Playwright end-to-end/visual tests (requires Chromium)",
    )
    args = parser.parse_args(argv)
    try:
        validate_environment()
        print(
            "Melodee prechecks: Git diffs, full repository gate, and documentation site.\n"
            "Checks stop at the first failure; source formatting is checked, not fixed.\n"
            + (
                "Playwright end-to-end/visual tests enabled."
                if args.include_e2e
                else "Playwright omitted; use --include-e2e for Admin UI changes."
            ),
            flush=True,
        )
        return run_checks(checks_for(include_e2e=args.include_e2e))
    except RuntimeError as error:
        print(f"Cannot run prechecks: {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("\nPrechecks interrupted.", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
