#!/usr/bin/env python3
"""Verify the operator command paths documented in the repository.

The check intentionally invokes only ``--help``.  It removes the process
environment except for PATH so a local ``.env`` or deployment credential
cannot turn a documentation audit into a database or network operation.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CLI = ROOT / "target" / "debug" / "melodee-cli"
CLI_COMMANDS: tuple[tuple[str, ...], ...] = (
    ("doctor",),
    ("migrate",),
    ("work",),
    ("work", "list"),
    ("work", "show"),
    ("work", "retry"),
    ("work", "seed-test"),
    ("work", "claim-test"),
    ("ingest",),
    ("ingest", "reconcile"),
    ("ingest", "inspect"),
    ("ingest", "normalize"),
    ("ingest", "validate"),
    ("ingest", "publish"),
    ("ingest", "manifest"),
    ("ingest", "publication"),
    ("ingest", "status"),
    ("catalog",),
    ("catalog", "import"),
    ("catalog", "status"),
    ("catalog", "inspect"),
    ("policy",),
    ("policy", "test"),
    ("policy", "evaluate"),
    ("policy", "inspect"),
    ("policy", "decide"),
    ("policy", "rules"),
    ("policy", "config"),
    ("legacy",),
    ("legacy", "inventory"),
    ("legacy", "plan"),
    ("legacy", "baseline"),
    ("legacy", "import"),
    ("legacy", "verify"),
    ("legacy", "delta"),
    ("legacy", "status"),
    ("legacy", "freeze"),
    ("search",),
    ("search", "status"),
    ("search", "rebuild"),
    ("search", "query"),
    ("search", "autocomplete"),
    ("search", "parse"),
    ("search", "explain"),
    ("charts",),
    ("charts", "import"),
    ("podcasts",),
    ("podcasts", "stuck"),
    ("podcasts", "recover"),
    ("worker-pools",),
    ("worker-pools", "list"),
    ("worker-pools", "pause"),
    ("worker-pools", "resume"),
    ("users",),
    ("users", "bootstrap-admin"),
    ("users", "disable"),
    ("users", "enable"),
    ("users", "import-state"),
    ("groups",),
    ("groups", "list"),
    ("groups", "create"),
    ("groups", "delete"),
    ("groups", "add-user"),
    ("groups", "remove-user"),
    ("groups", "bind-library"),
    ("groups", "unbind-library"),
    ("notifications",),
    ("notifications", "templates"),
    ("notifications", "templates", "list"),
    ("notifications", "test-send"),
    ("scripts",),
    ("scripts", "list"),
    ("scripts", "test"),
    ("config",),
    ("config", "list"),
    ("config", "validate"),
    ("config", "export"),
    ("config", "import"),
)


def sanitized_environment() -> dict[str, str]:
    """Return the minimum environment needed to launch a help command."""

    path = os.environ.get("PATH")
    if not path:
        raise RuntimeError("PATH is required to execute the command-help audit")
    return {"PATH": path}


def check_command(executable: Path, command: tuple[str, ...]) -> None:
    result = subprocess.run(
        [str(executable), *command, "--help"],
        cwd=ROOT,
        env=sanitized_environment(),
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise RuntimeError(
            f"{executable.name} {' '.join(command)} --help failed "
            f"with exit code {result.returncode}: {detail}"
        )
    if "Usage:" not in f"{result.stdout}\n{result.stderr}":
        raise RuntimeError(
            f"{executable.name} {' '.join(command)} --help did not emit Clap usage"
        )


def check_rehearsal_help() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "legacy_rehearsal.py"), "drill", "--help"],
        cwd=ROOT,
        env=sanitized_environment(),
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise RuntimeError(
            "legacy_rehearsal.py drill --help failed "
            f"with exit code {result.returncode}: {detail}"
        )
    if "usage:" not in f"{result.stdout}\n{result.stderr}".lower():
        raise RuntimeError("legacy_rehearsal.py drill --help did not emit usage")


def check_integration_probe_help() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "r4_integration_probe.py"), "--help"],
        cwd=ROOT,
        env=sanitized_environment(),
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise RuntimeError(
            "r4_integration_probe.py --help failed "
            f"with exit code {result.returncode}: {detail}"
        )
    if "usage:" not in f"{result.stdout}\n{result.stderr}".lower():
        raise RuntimeError("r4_integration_probe.py --help did not emit usage")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--cli",
        type=Path,
        default=DEFAULT_CLI,
        help="path to the built melodee-cli binary (default: target/debug/melodee-cli)",
    )
    args = parser.parse_args()
    if not args.cli.is_file() or args.cli.is_symlink():
        parser.error(f"--cli must be an existing regular file: {args.cli}")

    try:
        for command in CLI_COMMANDS:
            check_command(args.cli, command)
        check_rehearsal_help()
        check_integration_probe_help()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"command-help audit failed: {error}", file=sys.stderr)
        return 1

    print(
        f"command-help audit passed: {len(CLI_COMMANDS)} melodee-cli paths "
        "plus legacy_rehearsal.py drill and r4_integration_probe.py"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
