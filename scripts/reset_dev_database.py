#!/usr/bin/env python3
"""Destroy and recreate only the fixed local development PostgreSQL database."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
COMPOSE_FILE = "compose.dev.yaml"
PROJECT_NAME = "melodee-rs-dev"
EXPECTED_SERVICES = ["postgres"]
EXPECTED_VOLUMES = ["melodee_pg_data"]
DATABASE_URL = "postgres://melodee:melodee-dev@127.0.0.1:5433/melodee"


class ResetRefusedError(RuntimeError):
    """The development database topology does not match the destructive contract."""


def compose_command(*arguments: str) -> list[str]:
    return [
        "docker",
        "compose",
        "--project-name",
        PROJECT_NAME,
        "-f",
        COMPOSE_FILE,
        *arguments,
    ]


def capture_lines(command: list[str]) -> list[str]:
    result = subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return [line for line in result.stdout.splitlines() if line]


def reset_database() -> None:
    if not (ROOT / COMPOSE_FILE).is_file():
        raise ResetRefusedError(f"{COMPOSE_FILE} does not exist")

    services = capture_lines(compose_command("config", "--services"))
    volumes = capture_lines(compose_command("config", "--volumes"))
    if services != EXPECTED_SERVICES:
        raise ResetRefusedError(
            f"{COMPOSE_FILE} must declare only the {EXPECTED_SERVICES[0]} service"
        )
    if volumes != EXPECTED_VOLUMES:
        raise ResetRefusedError(
            f"{COMPOSE_FILE} must declare only the {EXPECTED_VOLUMES[0]} volume"
        )

    volume = f"{PROJECT_NAME}_{EXPECTED_VOLUMES[0]}"
    print(f"WARNING: deleting local development database volume {volume}.")
    print(
        "This permanently removes all users, catalog data, settings, and audit state "
        "in that database."
    )
    subprocess.run(
        compose_command("down", "--volumes", "--remove-orphans"),
        cwd=ROOT,
        check=True,
    )
    subprocess.run(
        compose_command("up", "-d", "--wait", EXPECTED_SERVICES[0]),
        cwd=ROOT,
        check=True,
    )

    print("Applying current Melodee migrations to the empty development database.")
    environment = os.environ.copy()
    environment["MELODEE__DATABASE__URL"] = DATABASE_URL
    subprocess.run(
        ["cargo", "run", "--locked", "-p", "melodee-cli", "--", "migrate"],
        cwd=ROOT,
        env=environment,
        check=True,
    )
    print("Development database reset complete. The schema is current and contains no administrator.")
    print("Create the first administrator before running 'melodee-cli config validate'.")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=(
            "This permanently deletes the melodee-rs-dev_melodee_pg_data Docker volume, "
            "recreates PostgreSQL, and applies current migrations."
        ),
    )
    return parser.parse_args()


def main() -> int:
    parse_args()
    try:
        reset_database()
    except ResetRefusedError as error:
        print(f"Refusing reset: {error}.", file=sys.stderr)
        return 1
    except FileNotFoundError as error:
        print(f"Required command not found: {error.filename}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
