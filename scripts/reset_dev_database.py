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
DATABASE_NAME = "melodee"
DATABASE_USER = "melodee"
MAINTENANCE_DATABASE = "postgres"
DATABASE_URL = "postgres://melodee:melodee-dev@127.0.0.1:5433/melodee"
DOCKER_VOLUME_NAME = f"{PROJECT_NAME}_{EXPECTED_VOLUMES[0]}"


class ResetRefusedError(RuntimeError):
    """The destructive reset cannot proceed safely."""


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


def postgres_command(*arguments: str) -> list[str]:
    return compose_command("exec", "-T", EXPECTED_SERVICES[0], *arguments)


def terminate_process(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    try:
        process.terminate()
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run_command(
    command: list[str],
    *,
    capture_output: bool = False,
    environment: dict[str, str] | None = None,
    timeout: float,
) -> str:
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        env=environment,
        stdout=subprocess.PIPE if capture_output else None,
        text=True,
    )
    try:
        stdout, _ = process.communicate(timeout=timeout)
    except (KeyboardInterrupt, subprocess.TimeoutExpired):
        terminate_process(process)
        raise
    if process.returncode != 0:
        raise subprocess.CalledProcessError(process.returncode, command, output=stdout)
    return stdout or ""


def capture_lines(command: list[str]) -> list[str]:
    output = run_command(command, capture_output=True, timeout=10)
    return [line for line in output.splitlines() if line]


def require_responsive_docker_volume_api() -> None:
    try:
        run_command(
            [
                "docker",
                "volume",
                "ls",
                "--quiet",
                "--filter",
                f"name={DOCKER_VOLUME_NAME}",
            ],
            capture_output=True,
            timeout=10,
        )
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        raise ResetRefusedError(
            "Docker volume operations are unavailable; restart Docker Engine or "
            "Docker Desktop, then retry"
        ) from error


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
    require_responsive_docker_volume_api()

    print(
        f"WARNING: dropping and recreating the local {DATABASE_NAME} development database.",
        flush=True,
    )
    print(
        "This permanently removes all users, catalog data, settings, and audit state "
        "in that database.",
        flush=True,
    )
    print("Starting the fixed development PostgreSQL service if needed.", flush=True)
    run_command(
        compose_command("up", "-d", "--wait", EXPECTED_SERVICES[0]),
        timeout=120,
    )

    print(
        f"Dropping and recreating database {DATABASE_NAME} without deleting its Docker volume.",
        flush=True,
    )
    run_command(
        postgres_command(
            "dropdb",
            "--if-exists",
            "--force",
            "--username",
            DATABASE_USER,
            "--maintenance-db",
            MAINTENANCE_DATABASE,
            DATABASE_NAME,
        ),
        timeout=60,
    )
    run_command(
        postgres_command(
            "createdb",
            "--username",
            DATABASE_USER,
            "--owner",
            DATABASE_USER,
            "--maintenance-db",
            MAINTENANCE_DATABASE,
            DATABASE_NAME,
        ),
        timeout=60,
    )

    print(
        "Applying current Melodee migrations to the empty development database.",
        flush=True,
    )
    environment = os.environ.copy()
    environment["MELODEE__DATABASE__URL"] = DATABASE_URL
    run_command(
        ["cargo", "run", "--locked", "-p", "melodee-cli", "--", "migrate"],
        environment=environment,
        timeout=900,
    )
    print(
        "Development database reset complete. "
        "The schema is current and contains no administrator.",
        flush=True,
    )
    print(
        "Create the first administrator before running 'melodee-cli config validate'.",
        flush=True,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=(
            "This permanently drops and recreates the melodee database inside the fixed "
            "development PostgreSQL container, preserves the Docker volume, and applies "
            "current migrations."
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
    except subprocess.TimeoutExpired as error:
        command = error.cmd[0] if isinstance(error.cmd, list) and error.cmd else "command"
        print(f"Timed out waiting for {command}; its process was terminated.", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        return error.returncode
    except KeyboardInterrupt:
        print("\nDevelopment database reset interrupted; child process terminated.", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
