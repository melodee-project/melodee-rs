#!/usr/bin/env python3
"""Validate a local Melodee release image without publishing it."""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUN_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,39}$")
SOURCE_LABEL = "https://github.com/melodee-project/melodee-rs"


class SmokeError(RuntimeError):
    """The image or disposable smoke environment violated the release contract."""


@dataclass(frozen=True)
class SmokeConfig:
    image: str
    release_version: str
    release_revision: str
    run_id: str
    port: int
    postgres_image: str

    @classmethod
    def from_environment(cls) -> SmokeConfig:
        image = os.environ.get("SMOKE_IMAGE", "")
        if not image:
            raise SmokeError("SMOKE_IMAGE must name the image to test")
        release_version = os.environ.get("RELEASE_VERSION", "")
        if not release_version:
            raise SmokeError("RELEASE_VERSION must match the image version")

        run_id = os.environ.get("SMOKE_RUN_ID", f"local-{os.getpid()}")
        if RUN_ID_RE.fullmatch(run_id) is None:
            raise SmokeError("SMOKE_RUN_ID contains unsupported characters or is too long")

        raw_port = os.environ.get("SMOKE_PORT", "18080")
        try:
            port = int(raw_port)
        except ValueError as error:
            raise SmokeError("SMOKE_PORT must be an integer between 1024 and 65535") from error
        if not 1024 <= port <= 65535:
            raise SmokeError("SMOKE_PORT must be an integer between 1024 and 65535")

        return cls(
            image=image,
            release_version=release_version,
            release_revision=os.environ.get("RELEASE_REVISION", ""),
            run_id=run_id,
            port=port,
            postgres_image=os.environ.get("POSTGRES_IMAGE", "postgres:18.4-alpine"),
        )


def run(
    command: list[str],
    *,
    capture: bool = False,
    quiet: bool = False,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=ROOT,
        check=check,
        text=True,
        stdout=subprocess.PIPE if capture else subprocess.DEVNULL if quiet else None,
        stderr=subprocess.DEVNULL if quiet else None,
    )


def captured(command: list[str]) -> str:
    return run(command, capture=True).stdout.strip()


def validate_labels(raw_labels: str, version: str, revision: str) -> None:
    try:
        labels = json.loads(raw_labels)
    except json.JSONDecodeError as error:
        raise SmokeError("image OCI labels are not valid JSON") from error
    if not isinstance(labels, dict):
        raise SmokeError("image has no OCI label map")

    expected = {
        "org.opencontainers.image.title": "Melodee",
        "org.opencontainers.image.source": SOURCE_LABEL,
        "org.opencontainers.image.version": version,
        "org.opencontainers.image.licenses": "MIT",
    }
    for name, value in expected.items():
        if labels.get(name) != value:
            raise SmokeError(f"image label {name} does not match {value!r}")
    if revision and labels.get("org.opencontainers.image.revision") != revision:
        raise SmokeError("image revision label does not match RELEASE_REVISION")


def ensure_port_available(port: int) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        try:
            probe.bind(("127.0.0.1", port))
        except OSError as error:
            raise SmokeError(f"refusing to use occupied host port {port}") from error


def fetch(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=3) as response:  # noqa: S310 - fixed localhost URL
        return response.read()


def fetch_when_ready(url: str, attempts: int = 30) -> bytes:
    for _ in range(attempts):
        try:
            return fetch(url)
        except (urllib.error.URLError, TimeoutError):
            time.sleep(1)
    raise SmokeError(f"service did not become ready at {url}")


class SmokeHarness:
    def __init__(self, config: SmokeConfig) -> None:
        self.config = config
        self.network = f"melodee-release-smoke-{config.run_id}"
        self.database = f"{self.network}-postgres"
        self.server = f"{self.network}-server"
        self.network_created = False
        self.database_created = False
        self.server_created = False

    def validate_names_and_environment(self) -> None:
        if any(len(name) > 63 for name in (self.network, self.database, self.server)):
            raise SmokeError("SMOKE_RUN_ID produces a Docker name longer than 63 characters")

        containers = set(captured(["docker", "ps", "-a", "--format", "{{.Names}}"]).splitlines())
        for name in (self.network, self.database, self.server):
            if name in containers:
                raise SmokeError(f"refusing to reuse existing Docker container: {name}")
        networks = set(captured(["docker", "network", "ls", "--format", "{{.Name}}"]).splitlines())
        if self.network in networks:
            raise SmokeError(f"refusing to reuse existing Docker network: {self.network}")
        ensure_port_available(self.config.port)

    def cleanup(self) -> None:
        if self.server_created:
            run(["docker", "rm", "-f", self.server], quiet=True, check=False)
        if self.database_created:
            run(["docker", "rm", "-f", self.database], quiet=True, check=False)
        if self.network_created:
            run(["docker", "network", "rm", self.network], quiet=True, check=False)

    def wait_for_database(self) -> None:
        command = [
            "docker",
            "exec",
            self.database,
            "pg_isready",
            "-U",
            "melodee",
            "-d",
            "melodee",
        ]
        for _ in range(30):
            if run(command, quiet=True, check=False).returncode == 0:
                return
            time.sleep(1)
        raise SmokeError("disposable PostgreSQL container did not become ready")

    def validate_image(self) -> str:
        image_id = captured(
            ["docker", "image", "inspect", "--format", "{{.Id}}", self.config.image]
        )
        if not image_id.startswith("sha256:"):
            raise SmokeError("image inspect did not return a sha256 image identifier")
        labels = captured(
            [
                "docker",
                "image",
                "inspect",
                "--format",
                "{{json .Config.Labels}}",
                self.config.image,
            ]
        )
        validate_labels(labels, self.config.release_version, self.config.release_revision)
        run(["docker", "image", "inspect", self.config.image], quiet=True)
        return image_id

    def start_database(self) -> None:
        run(["docker", "network", "create", self.network], quiet=True)
        self.network_created = True
        run(
            [
                "docker",
                "run",
                "-d",
                "--name",
                self.database,
                "--network",
                self.network,
                "-e",
                "POSTGRES_USER=melodee",
                "-e",
                "POSTGRES_PASSWORD=melodee-test",
                "-e",
                "POSTGRES_DB=melodee",
                self.config.postgres_image,
            ],
            quiet=True,
        )
        self.database_created = True
        self.wait_for_database()

    def validate_runtime_tools(self) -> None:
        user_id = captured(
            ["docker", "run", "--rm", "--entrypoint", "id", self.config.image, "-u"]
        )
        if user_id != "10001":
            raise SmokeError(f"image runs as unexpected user id {user_id!r}")

        for tool in ("ffmpeg", "ffprobe"):
            version = captured(
                ["docker", "run", "--rm", "--entrypoint", tool, self.config.image, "-version"]
            )
            print(version.splitlines()[0] if version else f"{tool}: no version output")
        run(
            ["docker", "run", "--rm", "--entrypoint", "fpcalc", self.config.image, "-version"]
        )
        cli_version = captured(
            [
                "docker",
                "run",
                "--rm",
                "--entrypoint",
                "melodee-cli",
                self.config.image,
                "--version",
            ]
        )
        if self.config.release_version not in cli_version:
            raise SmokeError("melodee-cli version does not match RELEASE_VERSION")

    def start_and_validate_server(self) -> None:
        port = self.config.port
        run(
            [
                "docker",
                "run",
                "-d",
                "--name",
                self.server,
                "--network",
                self.network,
                "-p",
                f"127.0.0.1:{port}:8080",
                "-e",
                f"MELODEE__DATABASE__URL=postgres://melodee:melodee-test@{self.database}:5432/melodee",
                "-e",
                f"MELODEE__NATIVE__PUBLIC_ORIGIN=http://127.0.0.1:{port}",
                "-e",
                "MELODEE__NATIVE__AUTH__JWT_KEY=release-smoke-native-jwt-key-at-least-thirty-two-bytes",
                "-e",
                "MELODEE__JELLYFIN__TOKEN_PEPPER=release-smoke-jellyfin-pepper-at-least-thirty-two-bytes",
                "-e",
                "MELODEE__INGEST__ENABLED=false",
                self.config.image,
            ],
            quiet=True,
        )
        self.server_created = True

        origin = f"http://127.0.0.1:{port}"
        fetch_when_ready(f"{origin}/readyz")
        health = json.loads(fetch(f"{origin}/healthz"))
        if health.get("status") != "ok" or health.get("version") != self.config.release_version:
            raise SmokeError("health response does not match the release image")
        readiness = json.loads(fetch(f"{origin}/readyz"))
        if readiness.get("status") != "ready" or readiness.get("version") != self.config.release_version:
            raise SmokeError("readiness response does not match the release image")
        admin_html = fetch(f"{origin}/admin/").decode("utf-8")
        if '<div id="root"' not in admin_html:
            raise SmokeError("admin application root was not served")

        run(["docker", "stop", "--time", "15", self.server], quiet=True)
        exit_code = captured(
            ["docker", "inspect", "--format", "{{.State.ExitCode}}", self.server]
        )
        if exit_code != "0":
            raise SmokeError(f"server exited with status {exit_code!r} after SIGTERM")

    def execute(self) -> None:
        self.validate_names_and_environment()
        image_id = self.validate_image()
        try:
            self.start_database()
            self.validate_runtime_tools()
            self.start_and_validate_server()
        finally:
            self.cleanup()
        print(
            f"container smoke passed: image={self.config.image} "
            f"image_id={image_id} version={self.config.release_version}"
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=(
            "Required environment: SMOKE_IMAGE, RELEASE_VERSION. Optional environment: "
            "RELEASE_REVISION, SMOKE_RUN_ID, SMOKE_PORT, POSTGRES_IMAGE. The harness creates "
            "uniquely named disposable Docker objects and removes them on exit."
        ),
    )
    return parser.parse_args()


def main() -> int:
    parse_args()
    try:
        config = SmokeConfig.from_environment()
        harness = SmokeHarness(config)

        def interrupt(_signum: int, _frame: object) -> None:
            raise KeyboardInterrupt

        signal.signal(signal.SIGINT, interrupt)
        signal.signal(signal.SIGTERM, interrupt)
        harness.execute()
    except SmokeError as error:
        print(str(error), file=sys.stderr)
        return 2
    except FileNotFoundError as error:
        print(f"Required command not found: {error.filename}", file=sys.stderr)
        return 2
    except subprocess.CalledProcessError as error:
        return error.returncode
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        print(f"invalid response from release image: {error}", file=sys.stderr)
        return 1
    except urllib.error.URLError as error:
        print(f"request to release image failed: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("container smoke interrupted", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
