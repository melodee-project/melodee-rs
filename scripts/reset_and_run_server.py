#!/usr/bin/env python3
"""Reset and launch a fresh local Melodee development system."""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import shutil
import socket
import subprocess
import sys
import threading
import time
from collections import deque
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence
from urllib.parse import urlsplit

import node_toolchain


ROOT = Path(__file__).resolve().parents[1]
ENV_FILE = ROOT / ".env"
ADMIN_UI = ROOT / "admin-ui"
RESET_HELPER = ROOT / "scripts" / "reset_dev_database.py"
RICH_REQUIREMENTS = ROOT / "scripts" / "requirements.txt"
SAFE_DATABASE_URL = "postgres://melodee:melodee-dev@127.0.0.1:5433/melodee"
DATABASE_ENV = "MELODEE__DATABASE__URL"
BOOTSTRAP_PASSWORD_ENV = "MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD"
SERVER_BIND_ENV = "MELODEE__SERVER__BIND"
LEGACY_SERVER_BIND_ENV = "MELODEE_BIND"
DEFAULT_SERVER_BIND = "127.0.0.1:8080"
SERVER_START_TIMEOUT_SECONDS = 900
COMMAND_OUTPUT_LINES = 30
ENV_KEY = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
DIRECTORY_ENVIRONMENTS = (
    ("MELODEE__LOGGING__DIRECTORY", "Daily logs"),
    ("MELODEE__INGEST__SOURCE_ROOT", "Inbound source"),
    ("MELODEE__INGEST__STAGING_ROOT", "Durable staging"),
    ("MELODEE__NORMALIZATION__WORK_AREA", "Normalization work area"),
    ("MELODEE__STORAGE__ROOT", "Canonical storage"),
    ("MELODEE__SEARCH__ROOT", "Search index"),
    ("MELODEE__PODCAST__STORAGE_ROOT", "Podcast storage"),
)
INGEST_SOURCES_ENV = "MELODEE__INGEST__SOURCES_JSON"
FORBIDDEN_WIPE_ROOTS = frozenset(
    Path(path)
    for path in (
        "/",
        "/etc",
        "/media",
        "/mnt",
        "/opt",
        "/srv",
        "/tmp",
        "/usr",
        "/var",
        "/var/tmp",
    )
)


class SetupError(RuntimeError):
    """The guided local setup cannot continue safely."""


class SetupCancelled(SetupError):
    """The operator declined the destructive reset."""


class CommandFailed(SetupError):
    """A setup child process failed."""

    def __init__(self, command: Sequence[str], returncode: int, output: str) -> None:
        super().__init__(
            f"command failed with exit code {returncode}: {shlex.join(command)}"
        )
        self.command = list(command)
        self.returncode = returncode
        self.output = output


@dataclass(frozen=True)
class BindAddress:
    """The configured listener and the address used for a local readiness probe."""

    configured: str
    host: str
    port: int

    @property
    def probe_host(self) -> str:
        if self.host == "0.0.0.0":
            return "127.0.0.1"
        if self.host == "::":
            return "::1"
        return self.host

    @property
    def browser_host(self) -> str:
        host = self.probe_host
        return f"[{host}]" if ":" in host else host


@dataclass(frozen=True)
class AdminCredentials:
    """Interactive first-administrator input."""

    username: str
    email: str
    password: str


@dataclass(frozen=True)
class ConfiguredDirectory:
    """One deduplicated, explicitly configured directory eligible for wiping."""

    path: Path
    sources: tuple[str, ...]


class RichUi:
    """Small Rich presentation boundary kept out of mutation and safety logic."""

    def __init__(self) -> None:
        from rich.console import Console
        from rich.panel import Panel
        from rich.prompt import Confirm, Prompt
        from rich.table import Table
        from rich.text import Text

        self.console = Console()
        self.error_console = Console(stderr=True)
        self.Panel = Panel
        self.Confirm = Confirm
        self.Prompt = Prompt
        self.Table = Table
        self.Text = Text

    def banner(self) -> None:
        body = self.Text()
        body.append("Fresh local setup\n", style="bold bright_cyan")
        body.append(
            "Optionally empty configured local roots, reset PostgreSQL, build the Admin UI "
            "and Rust binaries, start Melodee, and create the first administrator.",
            style="white",
        )
        self.console.print(
            self.Panel(
                body, title="MELODEE", border_style="bright_cyan", padding=(1, 2)
            )
        )

    def environment_summary(
        self, bind: BindAddress, directories: Sequence[ConfiguredDirectory]
    ) -> None:
        table = self.Table.grid(padding=(0, 2))
        table.add_column(style="bold cyan")
        table.add_column()
        table.add_row("Environment", str(ENV_FILE))
        table.add_row("Database", "melodee-rs-dev / melodee on 127.0.0.1:5433")
        table.add_row("Server", bind.configured)
        table.add_row("Admin UI", str(ADMIN_UI / "dist"))
        table.add_row("Configured roots", str(len(directories)))
        self.console.print(
            self.Panel(table, title="Validated local target", border_style="cyan")
        )

    def warn_reset(self) -> None:
        message = self.Text()
        message.append("This permanently removes ", style="bold yellow")
        message.append(
            "all users, catalog data, settings, and audit state from the fixed local "
            "melodee development database. The Docker volume is preserved; configured "
            "directory contents are handled by a separate confirmation."
        )
        self.console.print(
            self.Panel(message, title="Destructive reset", border_style="yellow")
        )

    def confirm_reset(self) -> bool:
        return bool(
            self.Confirm.ask(
                "[bold yellow]Delete and recreate the local melodee database?[/]",
                default=False,
                console=self.console,
            )
        )

    def warn_directory_wipe(
        self, directories: Sequence[ConfiguredDirectory]
    ) -> None:
        table = self.Table(show_header=True, header_style="bold yellow")
        table.add_column("Configuration")
        table.add_column("Directory")
        for directory in directories:
            table.add_row(", ".join(directory.sources), str(directory.path))
        self.console.print(
            self.Panel(
                table,
                title="Optional filesystem wipe",
                subtitle="Root directories remain; everything inside them is permanently deleted",
                border_style="yellow",
            )
        )

    def confirm_directory_wipe(self) -> bool:
        return bool(
            self.Confirm.ask(
                "[bold red]Delete every file and subdirectory in all configured roots?[/]",
                default=False,
                console=self.console,
            )
        )

    def stage(self, number: int, title: str, detail: str) -> None:
        self.console.print()
        self.console.print(f"[bold bright_cyan]{number}/7[/] [bold]{title}[/]")
        self.console.print(f"[dim]{detail}[/]")

    def status(self, message: str):
        return self.console.status(message, spinner="dots12")

    def success(self, message: str) -> None:
        self.console.print(f"[bold green]✓[/] {message}")

    def info(self, message: str) -> None:
        self.console.print(f"[bold cyan]•[/] {message}")

    def error(self, message: str) -> None:
        text = self.Text.assemble(("Error: ", "bold red"), message)
        self.error_console.print(text)

    def output_panel(self, title: str, output: str) -> None:
        text = self.Text(tail_output(output))
        self.error_console.print(self.Panel(text, title=title, border_style="red"))

    def prompt_credentials(self) -> AdminCredentials:
        self.console.print()
        self.console.print("[bold]Create the first administrator[/]")
        self.console.print(
            "[dim]The password is hidden, never placed in command arguments, and must match twice.[/]"
        )
        username = ""
        while not username:
            username = self.Prompt.ask(
                "Admin username", default="admin", console=self.console
            ).strip()
            if not username:
                self.error("Username cannot be blank.")

        suggested_email = f"{username}@example.test"
        email = ""
        while not email:
            email = self.Prompt.ask(
                "Admin email", default=suggested_email, console=self.console
            ).strip()
            if not email:
                self.error("Email cannot be blank.")

        while True:
            password = self.Prompt.ask(
                "Admin password", password=True, console=self.console
            )
            if not password.strip():
                self.error("Password cannot be blank.")
                continue
            confirmation = self.Prompt.ask(
                "Confirm password", password=True, console=self.console
            )
            if password == confirmation:
                return AdminCredentials(
                    username=username, email=email, password=password
                )
            self.error("Passwords did not match. Try again.")

    def ready(
        self, bind: BindAddress, username: str, environment: dict[str, str]
    ) -> None:
        origin = environment.get("MELODEE__NATIVE__PUBLIC_ORIGIN", "").rstrip("/")
        if not origin:
            origin = f"http://{bind.browser_host}:{bind.port}"
        table = self.Table.grid(padding=(0, 2))
        table.add_column(style="bold green")
        table.add_column()
        table.add_row("Admin UI", f"{origin}/admin")
        table.add_row("Username", username)
        table.add_row("Server", "running; press Ctrl+C to stop")
        self.console.print()
        self.console.print(
            self.Panel(table, title="Fresh Melodee is ready", border_style="green")
        )

    def server_log(self, line: str) -> None:
        text = self.Text()
        text.append("server  ", style="dim cyan")
        text.append(line)
        self.console.print(text)


class ServerMonitor:
    """Drains server output continuously and retains a bounded diagnostic tail."""

    def __init__(self, process: subprocess.Popen[str], ui: RichUi) -> None:
        self.process = process
        self.ui = ui
        self.lines: deque[str] = deque(maxlen=COMMAND_OUTPUT_LINES)
        self.forward = threading.Event()
        self.thread = threading.Thread(
            target=self._drain, name="melodee-server-log", daemon=True
        )
        self.thread.start()

    def _drain(self) -> None:
        if self.process.stdout is None:
            return
        for raw_line in self.process.stdout:
            line = raw_line.rstrip("\r\n")
            self.lines.append(line)
            if self.forward.is_set():
                self.ui.server_log(line)

    def tail(self) -> str:
        return "\n".join(self.lines)


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=(
            "Safety: this command accepts only the fixed compose.dev.yaml development database, "
            "preserves the Docker volume, separately confirms any configured-directory wipe, "
            "and refuses to run without .env."
        ),
    )
    parser.add_argument(
        "--yes",
        action="store_true",
        help=(
            "skip only the destructive database confirmation; the configured-directory wipe "
            "and administrator prompts remain interactive"
        ),
    )
    return parser.parse_args(argv)


def parse_environment_file(
    path: Path, base_environment: dict[str, str] | None = None
) -> dict[str, str]:
    """Parse shell-compatible KEY=VALUE lines without executing the environment file."""
    if not path.is_file():
        raise SetupError(f"required environment file was not found: {path}")

    environment = dict(base_environment or {})
    for line_number, original in enumerate(
        path.read_text(encoding="utf-8").splitlines(), start=1
    ):
        line = original.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line.removeprefix("export ").lstrip()
        key, separator, raw_value = line.partition("=")
        key = key.strip()
        if not separator or not ENV_KEY.fullmatch(key):
            raise SetupError(f"invalid .env assignment on line {line_number}")

        lexer = shlex.shlex(raw_value, posix=True)
        lexer.whitespace_split = True
        lexer.commenters = "#"
        try:
            values = list(lexer)
        except ValueError as error:
            raise SetupError(f"invalid .env quoting on line {line_number}") from error
        if len(values) > 1:
            raise SetupError(f"unquoted whitespace in .env value on line {line_number}")
        environment[key] = values[0] if values else ""
    return environment


def validate_development_environment(environment: dict[str, str]) -> None:
    if environment.get(DATABASE_ENV) != SAFE_DATABASE_URL:
        raise SetupError(
            f"{DATABASE_ENV} must match the fixed compose.dev.yaml development database; "
            "refusing to reset any other target"
        )
    if not RESET_HELPER.is_file():
        raise SetupError(f"database reset helper was not found: {RESET_HELPER}")
    if not (ADMIN_UI / "pnpm-lock.yaml").is_file():
        raise SetupError("admin-ui/pnpm-lock.yaml was not found")


def _validate_configured_path(raw_path: str, source: str) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise SetupError(f"{source} must be an absolute path before it can be wiped")
    candidate = Path(os.path.normpath(candidate))
    home = Path.home()
    if candidate in FORBIDDEN_WIPE_ROOTS or candidate == home:
        raise SetupError(f"{source} targets an unsafe broad directory: {candidate}")
    if candidate == ROOT or candidate.is_relative_to(ROOT) or ROOT.is_relative_to(candidate):
        raise SetupError(
            f"{source} overlaps the repository checkout and cannot be wiped: {candidate}"
        )
    if home.is_relative_to(candidate):
        raise SetupError(
            f"{source} contains the current home directory and cannot be wiped: {candidate}"
        )

    current = Path(candidate.anchor)
    for component in candidate.parts[1:]:
        current /= component
        if current.is_symlink():
            raise SetupError(
                f"{source} contains a symbolic link component and cannot be wiped: {current}"
            )
    if candidate.exists() and not candidate.is_dir():
        raise SetupError(f"{source} is not a directory: {candidate}")
    return candidate


def configured_directories(
    file_environment: dict[str, str],
) -> list[ConfiguredDirectory]:
    """Resolve only directory roots explicitly assigned by the repository .env."""
    candidates: list[tuple[str, str]] = []
    for key, _label in DIRECTORY_ENVIRONMENTS:
        value = file_environment.get(key, "").strip()
        if value:
            candidates.append((key, value))

    sources_value = file_environment.get(INGEST_SOURCES_ENV, "").strip()
    if sources_value:
        try:
            sources = json.loads(sources_value)
        except json.JSONDecodeError as error:
            raise SetupError(f"{INGEST_SOURCES_ENV} is not valid JSON") from error
        if not isinstance(sources, list):
            raise SetupError(f"{INGEST_SOURCES_ENV} must contain a JSON array")
        for index, source in enumerate(sources):
            if not isinstance(source, dict):
                raise SetupError(f"{INGEST_SOURCES_ENV}[{index}] must be an object")
            root = source.get("root")
            if not isinstance(root, str) or not root.strip():
                raise SetupError(
                    f"{INGEST_SOURCES_ENV}[{index}].root must be a non-empty string"
                )
            candidates.append((f"{INGEST_SOURCES_ENV}[{index}].root", root.strip()))

    deduplicated: dict[Path, list[str]] = {}
    for source, raw_path in candidates:
        path = _validate_configured_path(raw_path, source)
        deduplicated.setdefault(path, []).append(source)

    directories = [
        ConfiguredDirectory(path=path, sources=tuple(sources))
        for path, sources in sorted(deduplicated.items(), key=lambda item: str(item[0]))
    ]
    for index, directory in enumerate(directories):
        for other in directories[index + 1 :]:
            if directory.path in other.path.parents or other.path in directory.path.parents:
                raise SetupError(
                    "configured wipe roots overlap; use distinct directories: "
                    f"{directory.path} and {other.path}"
                )
    return directories


def embedded_mount_point(root: Path) -> Path | None:
    """Return the first mount nested below root without following symbolic links."""
    if not root.exists():
        return None
    for current, directory_names, _file_names in os.walk(
        root, topdown=True, followlinks=False
    ):
        retained: list[str] = []
        for name in directory_names:
            candidate = Path(current) / name
            if candidate.is_symlink():
                continue
            if candidate.is_mount():
                return candidate
            retained.append(name)
        directory_names[:] = retained
    return None


def wipe_configured_directories(
    directories: Sequence[ConfiguredDirectory],
) -> int:
    """Empty validated roots while preserving the root directories themselves."""
    validated: list[ConfiguredDirectory] = []
    try:
        for directory in directories:
            source = ", ".join(directory.sources)
            path = _validate_configured_path(str(directory.path), source)
            mount = embedded_mount_point(path)
            if mount is not None:
                raise SetupError(
                    f"configured directory contains an embedded mount point: {mount}"
                )
            validated.append(ConfiguredDirectory(path=path, sources=directory.sources))
    except OSError as error:
        raise SetupError(f"could not preflight configured directories: {error}") from error

    removed = 0
    for directory in validated:
        try:
            directory.path.mkdir(parents=True, exist_ok=True)
            entries = list(directory.path.iterdir())
            for entry in entries:
                if entry.is_symlink() or not entry.is_dir():
                    entry.unlink()
                else:
                    shutil.rmtree(entry)
                removed += 1
            if any(directory.path.iterdir()):
                raise SetupError(
                    f"configured directory was not empty after wiping: {directory.path}"
                )
        except SetupError:
            raise
        except OSError as error:
            raise SetupError(
                f"could not wipe configured directory {directory.path}: {error}"
            ) from error
    return removed


def confirm_destructive_actions(
    ui: RichUi,
    *,
    assume_database_yes: bool,
    directories: Sequence[ConfiguredDirectory],
) -> bool:
    """Collect both destructive decisions before any local state is changed."""
    ui.warn_reset()
    if not assume_database_yes and not ui.confirm_reset():
        raise SetupCancelled("database reset cancelled; no data was changed")
    if not directories:
        return False
    ui.warn_directory_wipe(directories)
    return ui.confirm_directory_wipe()


def parse_bind_address(environment: dict[str, str]) -> BindAddress:
    configured = environment.get(
        SERVER_BIND_ENV,
        environment.get(LEGACY_SERVER_BIND_ENV, DEFAULT_SERVER_BIND),
    ).strip()
    try:
        parsed = urlsplit(f"tcp://{configured}")
        port = parsed.port
    except ValueError as error:
        raise SetupError(f"{SERVER_BIND_ENV} is not a valid host:port value") from error
    if (
        not parsed.hostname
        or port is None
        or parsed.username is not None
        or parsed.password is not None
        or parsed.path
        or parsed.query
        or parsed.fragment
    ):
        raise SetupError(f"{SERVER_BIND_ENV} is not a valid host:port value")
    return BindAddress(configured=configured, host=parsed.hostname, port=port)


def require_commands(environment: dict[str, str]) -> list[str]:
    search_path = os.pathsep.join(os.get_exec_path(environment))
    missing = [
        command
        for command in ("cargo", "docker")
        if shutil.which(command, path=search_path) is None
    ]
    if missing:
        raise SetupError(f"missing required command(s): {', '.join(missing)}")
    try:
        return node_toolchain.command(ADMIN_UI, environment=environment)
    except RuntimeError as error:
        raise SetupError(str(error)) from error


def ensure_server_port_available(bind: BindAddress) -> None:
    try:
        connection = socket.create_connection(
            (bind.probe_host, bind.port), timeout=0.25
        )
    except socket.gaierror as error:
        raise SetupError(
            f"cannot resolve configured server host {bind.host}"
        ) from error
    except (ConnectionRefusedError, TimeoutError, OSError):
        return
    connection.close()
    raise SetupError(
        f"server address {bind.configured} is already accepting connections; "
        "stop the existing process before resetting its database"
    )


def reset_command() -> list[str]:
    return [sys.executable, str(RESET_HELPER)]


def admin_ui_commands(pnpm_prefix: Sequence[str]) -> list[list[str]]:
    return [
        [*pnpm_prefix, "install", "--frozen-lockfile"],
        [*pnpm_prefix, "build"],
    ]


def rust_build_command() -> list[str]:
    return [
        "cargo",
        "build",
        "--locked",
        "-p",
        "melodee-server",
        "-p",
        "melodee-cli",
        "-p",
        "melodee-application",
    ]


def binary_path(name: str) -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return ROOT / "target" / "debug" / f"{name}{suffix}"


def admin_bootstrap_command(
    executable: Path, credentials: AdminCredentials
) -> list[str]:
    return [
        str(executable),
        "users",
        "bootstrap-admin",
        "--username",
        credentials.username,
        "--email",
        credentials.email,
    ]


def terminate_process(process: subprocess.Popen[str], *, timeout: float = 10) -> None:
    """Terminate and reap a child process after cancellation or timeout."""
    if process.poll() is not None:
        return
    try:
        process.terminate()
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run_checked(
    command: Sequence[str],
    *,
    cwd: Path,
    environment: dict[str, str],
    timeout: float,
) -> str:
    process = subprocess.Popen(
        list(command),
        cwd=cwd,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        output, _ = process.communicate(timeout=timeout)
    except KeyboardInterrupt:
        terminate_process(process)
        raise
    except subprocess.TimeoutExpired as error:
        output = error.stdout or ""
        if isinstance(output, bytes):
            output = output.decode(errors="replace")
        terminate_process(process)
        raise CommandFailed(
            command, 124, f"{output}\nTimed out after {timeout:.0f} seconds."
        ) from error
    if process.returncode != 0:
        raise CommandFailed(command, process.returncode, output)
    return output


def run_stage(
    ui: RichUi,
    number: int,
    title: str,
    detail: str,
    success: str,
    command: Sequence[str],
    *,
    cwd: Path,
    environment: dict[str, str],
    timeout: float,
) -> str:
    ui.stage(number, title, detail)
    with ui.status(f"{title}…"):
        output = run_checked(
            command,
            cwd=cwd,
            environment=environment,
            timeout=timeout,
        )
    ui.success(success)
    return output


def launch_server(
    executable: Path, environment: dict[str, str], ui: RichUi
) -> ServerMonitor:
    if not executable.is_file():
        raise SetupError(f"server executable was not built: {executable}")
    process = subprocess.Popen(
        [str(executable)],
        cwd=ROOT,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    return ServerMonitor(process, ui)


def wait_for_server(monitor: ServerMonitor, bind: BindAddress) -> None:
    deadline = time.monotonic() + SERVER_START_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        returncode = monitor.process.poll()
        if returncode is not None:
            raise CommandFailed(
                [str(binary_path("melodee-server"))],
                returncode,
                monitor.tail(),
            )
        try:
            connection = socket.create_connection(
                (bind.probe_host, bind.port), timeout=0.25
            )
        except (ConnectionRefusedError, TimeoutError, OSError):
            time.sleep(0.2)
            continue
        connection.close()
        return
    raise CommandFailed(
        [str(binary_path("melodee-server"))],
        124,
        f"{monitor.tail()}\nServer did not listen on {bind.configured} within "
        f"{SERVER_START_TIMEOUT_SECONDS} seconds.",
    )


def stop_server(monitor: ServerMonitor) -> None:
    terminate_process(monitor.process, timeout=20)
    monitor.thread.join(timeout=2)


def follow_server(monitor: ServerMonitor, ui: RichUi) -> int:
    monitor.forward.set()
    try:
        while monitor.process.poll() is None:
            time.sleep(0.5)
    except KeyboardInterrupt:
        ui.info("Stopping Melodee…")
        return 0
    return monitor.process.returncode or 0


def tail_output(output: str) -> str:
    lines = output.rstrip().splitlines()
    if not lines:
        return "No command output was captured."
    omitted = max(0, len(lines) - COMMAND_OUTPUT_LINES)
    tail = lines[-COMMAND_OUTPUT_LINES:]
    if omitted:
        tail.insert(0, f"… {omitted} earlier lines omitted …")
    return "\n".join(tail)


def run_setup(ui: RichUi, *, assume_yes: bool) -> int:
    file_environment = parse_environment_file(ENV_FILE)
    environment = dict(os.environ)
    environment.update(file_environment)
    environment.pop(BOOTSTRAP_PASSWORD_ENV, None)
    validate_development_environment(environment)
    directories = configured_directories(file_environment)
    bind = parse_bind_address(environment)
    pnpm_prefix = require_commands(environment)
    ensure_server_port_available(bind)

    ui.banner()
    ui.stage(
        1,
        "Validate local configuration",
        "Checking .env, tools, database target, and server port.",
    )
    ui.environment_summary(bind, directories)
    ui.success("The local-development target is exact and available.")
    wipe_directories = confirm_destructive_actions(
        ui, assume_database_yes=assume_yes, directories=directories
    )

    ui.stage(
        2,
        "Prepare configured directories",
        "Emptying explicitly configured Melodee roots while preserving each root directory."
        if wipe_directories
        else "The optional directory wipe was not selected; configured contents are preserved.",
    )
    if wipe_directories:
        with ui.status("Wiping configured directory contents…"):
            removed = wipe_configured_directories(directories)
        ui.success(
            f"Removed {removed} top-level entries from {len(directories)} configured roots."
        )
    else:
        ui.info("Configured directory contents were not changed.")

    run_stage(
        ui,
        3,
        "Reset PostgreSQL",
        "Using the repository's guarded reset helper to recreate melodee and apply migrations.",
        "The development database is empty and fully migrated.",
        reset_command(),
        cwd=ROOT,
        environment=environment,
        timeout=1200,
    )

    install_command, build_command = admin_ui_commands(pnpm_prefix)
    run_stage(
        ui,
        4,
        "Install Admin UI dependencies",
        "Installing exactly the versions recorded in admin-ui/pnpm-lock.yaml.",
        "Admin UI dependencies match the lockfile.",
        install_command,
        cwd=ADMIN_UI,
        environment=environment,
        timeout=900,
    )
    run_stage(
        ui,
        4,
        "Build the Admin UI",
        "Creating the production SPA served from admin-ui/dist.",
        "The Admin UI production build is ready.",
        build_command,
        cwd=ADMIN_UI,
        environment=environment,
        timeout=900,
    )

    run_stage(
        ui,
        5,
        "Build Melodee",
        "Building locked debug executables for the server and CLI.",
        "The server and CLI executables are ready.",
        rust_build_command(),
        cwd=ROOT,
        environment=environment,
        timeout=1200,
    )

    ui.stage(
        6, "Start the Melodee server", f"Waiting for the listener at {bind.configured}."
    )
    with ui.status("Starting Melodee…"):
        monitor = launch_server(binary_path("melodee-server"), environment, ui)
        try:
            wait_for_server(monitor, bind)
        except BaseException:
            stop_server(monitor)
            raise
    ui.success(f"Melodee is listening at {bind.configured}.")

    try:
        ui.stage(
            7,
            "Create the first administrator",
            "The CLI writes the first account only when the fresh database has no administrator.",
        )
        credentials = ui.prompt_credentials()
        cli_executable = binary_path("melodee-cli")
        if not cli_executable.is_file():
            raise SetupError(f"CLI executable was not built: {cli_executable}")
        bootstrap_environment = environment.copy()
        bootstrap_environment[BOOTSTRAP_PASSWORD_ENV] = credentials.password
        try:
            with ui.status("Creating the first administrator…"):
                run_checked(
                    admin_bootstrap_command(cli_executable, credentials),
                    cwd=ROOT,
                    environment=bootstrap_environment,
                    timeout=300,
                )
        finally:
            bootstrap_environment.pop(BOOTSTRAP_PASSWORD_ENV, None)
        ui.success(f"Administrator '{credentials.username}' was created.")
        ui.ready(bind, credentials.username, environment)
        return follow_server(monitor, ui)
    finally:
        stop_server(monitor)


def display_startup_error(message: str) -> None:
    try:
        from rich.console import Console
        from rich.text import Text

        Console(stderr=True).print(Text.assemble(("Error: ", "bold red"), message))
    except ModuleNotFoundError:
        print(f"Error: {message}", file=sys.stderr)


def load_ui() -> RichUi:
    try:
        return RichUi()
    except ModuleNotFoundError as error:
        if error.name == "rich" or (error.name and error.name.startswith("rich.")):
            raise SetupError(
                f"Rich is required; install it with '{sys.executable} -m pip install -r "
                f"{RICH_REQUIREMENTS.relative_to(ROOT)}'"
            ) from error
        raise


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    if not ENV_FILE.is_file():
        display_startup_error(
            f"{ENV_FILE} was not found. Copy .env.example to .env and configure it before running setup."
        )
        return 2

    try:
        ui = load_ui()
    except SetupError as error:
        display_startup_error(str(error))
        return 1

    try:
        return run_setup(ui, assume_yes=args.yes)
    except SetupCancelled as error:
        ui.info(str(error))
        return 0
    except CommandFailed as error:
        ui.error(str(error))
        ui.output_panel("Command output", error.output)
        return error.returncode if error.returncode > 0 else 1
    except SetupError as error:
        ui.error(str(error))
        return 1
    except FileNotFoundError as error:
        ui.error(f"required command was not found: {error.filename}")
        return 1
    except KeyboardInterrupt:
        ui.info("Setup interrupted.")
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
