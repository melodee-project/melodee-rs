#!/usr/bin/env python3
"""Start or stop the local PostgreSQL/server/Admin UI stack without resetting data."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
import hmac
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import time
from urllib.error import URLError
from urllib.request import ProxyHandler, build_opener

import reset_and_run_server as setup

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = ROOT / ".local-dev"
STATE = RUNTIME / "server.json"
LOG = RUNTIME / "server.log"
START_TIMEOUT = 120
DATABASE_START_TIMEOUT = 600
STOP_TIMEOUT = 40
CORS_ENV = "MELODEE__OPENSUBSONIC__CORS__ALLOWED_ORIGINS"


class StackError(RuntimeError):
    """An operation cannot complete safely."""


@contextmanager
def command_lock():
    """OS locks release after a crash; a stale PID never authorizes a signal."""
    RUNTIME.mkdir(mode=0o700, exist_ok=True)
    with (RUNTIME / "command.lock").open("a+b") as handle:
        if handle.tell() == 0:
            handle.write(b"\0")
            handle.flush()
        handle.seek(0)
        try:
            if os.name == "nt":
                import msvcrt

                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl

                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise StackError("another make up/down command is still running") from error
        yield


def run(command: list[str], *, cwd: Path = ROOT, environment=None, capture=False):
    print("$ " + " ".join(command), flush=True)
    return subprocess.run(
        command,
        cwd=cwd,
        env=environment,
        check=True,
        shell=False,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        timeout=1200,
    ).stdout


def compose(*arguments: str) -> list[str]:
    return [
        "docker",
        "compose",
        "--project-name",
        "melodee-rs-dev",
        "-f",
        str(ROOT / "compose.dev.yaml"),
        *arguments,
    ]


def validate_compose() -> None:
    """Fail closed before starting/stopping anything if the fixed target drifts."""
    config = json.loads(run(compose("config", "--format", "json"), capture=True))
    services = config.get("services", {})
    postgres = services.get("postgres", {})
    ports = postgres.get("ports", [])
    mounts = postgres.get("volumes", [])
    volumes = config.get("volumes", {})
    if (
        config.get("name") != "melodee-rs-dev"
        or set(services) != {"postgres"}
        or set(volumes) != {"melodee_pg_data"}
        or volumes["melodee_pg_data"].get("name") != "melodee-rs-dev_melodee_pg_data"
        or volumes["melodee_pg_data"].get("external", False)
        or postgres.get("environment")
        != {
            "POSTGRES_DB": "melodee",
            "POSTGRES_USER": "melodee",
            "POSTGRES_PASSWORD": "melodee-dev",
        }
        or len(ports) != 1
        or str(ports[0].get("published")) != "5433"
        or ports[0].get("target") != 5432
        or len(mounts) != 1
        or mounts[0].get("type") != "volume"
        or mounts[0].get("source") != "melodee_pg_data"
        or mounts[0].get("target") != "/var/lib/postgresql"
    ):
        raise StackError(
            "compose.dev.yaml no longer describes the fixed development database"
        )


def load_environment() -> dict[str, str]:
    environment = setup.parse_environment_file(ROOT / ".env")
    # Match the server/CLI's dotenv behavior: exported variables take precedence.
    environment.update(os.environ)
    environment.pop(setup.BOOTSTRAP_PASSWORD_ENV, None)
    if environment.get(setup.DATABASE_ENV) != setup.SAFE_DATABASE_URL:
        raise StackError(
            f"{setup.DATABASE_ENV} must select the compose.dev.yaml development database"
        )
    config = json.loads(run(compose("config", "--format", "json"), capture=True))
    defaults = config.get("x-melodee-server-environment", {})
    # Only the documented browser setting can enter the host process through
    # Compose defaults; database and process-control settings stay explicit.
    if (
        not isinstance(defaults, dict)
        or set(defaults) - {CORS_ENV}
        or any(not isinstance(value, str) for value in defaults.values())
    ):
        raise StackError("invalid x-melodee-server-environment in compose.dev.yaml")
    for key, value in defaults.items():
        environment.setdefault(key, value)
    # Build/run the checkout's SPA and binaries even if another checkout was exported.
    environment["MELODEE__ADMIN_UI__DIST"] = str(ROOT / "admin-ui" / "dist")
    environment["CARGO_TARGET_DIR"] = str(ROOT / "target")
    return environment


def read_state() -> dict | None:
    if not STATE.exists():
        return None
    state = json.loads(STATE.read_text(encoding="utf-8"))
    if (
        not isinstance(state, dict)
        or state.get("root") != str(ROOT)
        or type(state.get("port")) is not int
        or not 0 < state["port"] < 65536
        or not isinstance(state.get("token"), str)
        or len(state["token"]) != 64
    ):
        raise StackError(
            f"invalid ownership state: {STATE}; refusing to stop any process"
        )
    return state


def control(state: dict, action: str) -> dict | None:
    """Talk only to the authenticated supervisor; never signal a saved PID."""
    try:
        connection = socket.create_connection(("127.0.0.1", state["port"]), timeout=2)
    except ConnectionRefusedError:
        return None
    with connection, connection.makefile("rb") as response:
        connection.sendall(
            json.dumps({"token": state["token"], "action": action}).encode() + b"\n"
        )
        reply = json.loads(response.readline(4096))
    if not isinstance(reply, dict) or not hmac.compare_digest(
        str(reply.get("token", "")).encode(), state["token"].encode()
    ):
        raise StackError(
            "local supervisor identity mismatch; refusing to stop any process"
        )
    return reply


def stop_server() -> None:
    state = read_state()
    if state is None:
        return
    # Prove the endpoint's identity before issuing a shutdown request. A stale
    # control port may now belong to an unrelated local service.
    if control(state, "status") is None:
        STATE.unlink(missing_ok=True)
        return
    control(state, "stop")
    deadline = time.monotonic() + STOP_TIMEOUT
    while time.monotonic() < deadline:
        if not STATE.exists():
            return
        time.sleep(0.1)
    raise StackError(f"server did not stop; PostgreSQL was left running. See {LOG}")


def supervise(environment: dict[str, str], command: list[str] | None = None) -> None:
    """Keep the actual child handle until shutdown, including failed startup."""
    token = secrets.token_hex(32)
    process = None

    def interrupted(_signal, _frame):
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, interrupted)
    try:
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(5)
            listener.settimeout(0.2)
            # Debug is the default (fast incremental builds), but image
            # processing and audio analysis are dramatically slower unoptimised:
            # a 1024px cover takes ~3.4s in debug and ~0.11s in release, which
            # reads as an application defect during client parity testing. Set
            # MELODEE_DEV_PROFILE=release (after `cargo build --release
            # -p melodee-server`) to run the real thing.
            profile = (
                environment.get("MELODEE_DEV_PROFILE") or "debug"
            ).strip().lower()
            if profile not in ("debug", "release"):
                raise StackError(
                    f"MELODEE_DEV_PROFILE must be 'debug' or 'release', got {profile!r}"
                )
            executable = (
                ROOT
                / "target"
                / profile
                / ("melodee-server.exe" if os.name == "nt" else "melodee-server")
            )
            # An explicit command (tests, tooling) bypasses the binary contract.
            if command is None and not executable.is_file():
                raise StackError(
                    f"{executable} is missing; build it first with "
                    f"`cargo build --profile {profile} -p melodee-server`"
                )
            process = subprocess.Popen(
                command or [str(executable)],
                cwd=ROOT,
                env=environment,
                stdin=subprocess.DEVNULL,
            )
            state = {
                "root": str(ROOT),
                "port": listener.getsockname()[1],
                "token": token,
            }
            temporary = STATE.with_suffix(".tmp")
            with temporary.open("w", encoding="utf-8") as handle:
                os.chmod(temporary, 0o600)
                json.dump(state, handle)
            temporary.replace(STATE)
            while process.poll() is None:
                try:
                    connection, _address = listener.accept()
                except TimeoutError:
                    continue
                try:
                    with connection, connection.makefile("rb") as request:
                        connection.settimeout(1)
                        message = json.loads(request.readline(4096))
                        if not isinstance(message, dict) or not hmac.compare_digest(
                            str(message.get("token", "")).encode(), token.encode()
                        ):
                            continue
                        action = message.get("action")
                        if action not in ("status", "stop"):
                            continue
                        connection.sendall(
                            json.dumps(
                                {"token": token, "running": process.poll() is None}
                            ).encode()
                            + b"\n"
                        )
                        if action == "stop":
                            break
                except (OSError, ValueError, RecursionError):
                    continue
            if process.poll() is not None:
                print(f"Melodee exited with code {process.returncode}", flush=True)
    finally:
        if process is not None:
            setup.terminate_process(process, timeout=20)
        state = read_state()
        if state and state["token"] == token:
            STATE.unlink(missing_ok=True)


def ready(origin: str) -> bool:
    try:
        with build_opener(ProxyHandler({})).open(
            origin + "/readyz", timeout=2
        ) as response:
            return response.status == 200
    except (OSError, URLError):
        return False


def wait_for_postgres() -> None:
    """Large existing volumes can outlast Compose's initial healthcheck retries."""
    print("Waiting for development PostgreSQL (up to 10 minutes)…", flush=True)
    deadline = time.monotonic() + DATABASE_START_TIMEOUT
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(
                compose(
                    "exec",
                    "-T",
                    "postgres",
                    "pg_isready",
                    "-U",
                    "melodee",
                    "-d",
                    "melodee",
                ),
                cwd=ROOT,
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
            if result.returncode == 0:
                return
        except subprocess.TimeoutExpired:
            pass
        time.sleep(1)
    raise StackError(
        "development PostgreSQL did not become ready; inspect "
        "docker compose -f compose.dev.yaml logs postgres"
    )


def up() -> None:
    environment = load_environment()
    bind = setup.parse_bind_address(environment)
    if bind.port == 0:
        raise StackError("configure a fixed nonzero server port for make up")
    origin = f"http://{bind.browser_host}:{bind.port}"
    state = read_state()
    if state and control(state, "status"):
        validate_compose()
        run(compose("up", "-d", "postgres"))
        wait_for_postgres()
        if not ready(origin):
            raise StackError(
                f"the managed server is not ready at {origin}; see {LOG}. "
                "Use make down/up if configuration changed."
            )
        print(
            "Melodee is already running. Use make down before rebuilding or changing .env."
        )
        return
    STATE.unlink(missing_ok=True)
    for tool in ("cargo", "docker", "pnpm", "ffmpeg", "ffprobe", "fpcalc"):
        if shutil.which(tool, path=environment.get("PATH")) is None:
            raise StackError(f"required command not found: {tool}")
    # Bind, rather than kill anything using the requested port.
    with socket.socket(
        socket.AF_INET6 if ":" in bind.host else socket.AF_INET
    ) as probe:
        # Permit immediate restarts while old HTTP connections are in TIME_WAIT.
        # A live listener still prevents this bind (SO_REUSEPORT is not enabled).
        if os.name != "nt":
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            probe.bind((bind.host, bind.port))
        except OSError as error:
            raise StackError(
                f"server address {bind.configured} is unavailable; stop its owner first"
            ) from error
    validate_compose()
    for command in setup.admin_ui_commands(["pnpm"]):
        run(command, cwd=ROOT / "admin-ui", environment=environment)
    run(setup.rust_build_command(), environment=environment)
    was_running = (
        "postgres"
        in run(
            compose("ps", "--status", "running", "--services"), capture=True
        ).splitlines()
    )
    supervisor = None
    try:
        run(compose("up", "-d", "postgres"))
        wait_for_postgres()
        options = (
            {
                "creationflags": subprocess.CREATE_NEW_PROCESS_GROUP
                | subprocess.DETACHED_PROCESS
            }
            if os.name == "nt"
            else {"start_new_session": True}
        )
        with LOG.open("a", encoding="utf-8") as output:
            supervisor = subprocess.Popen(
                [sys.executable, str(Path(__file__).resolve()), "_supervise"],
                cwd=ROOT,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=output,
                stderr=subprocess.STDOUT,
                **options,
            )
        deadline = time.monotonic() + START_TIMEOUT
        while time.monotonic() < deadline:
            if supervisor.poll() is not None:
                raise StackError(f"server startup failed; see {LOG}")
            state = read_state()
            if state and control(state, "status") and ready(origin):
                print(
                    f"Admin UI: {origin}/admin\nServer log: {LOG}\nStop everything with: make down"
                )
                print(
                    "For a new database, create your first administrator with melodee-cli users bootstrap-admin (see README.md)."
                )
                return
            time.sleep(0.2)
        raise StackError(f"server readiness timed out; see {LOG}")
    except BaseException:
        if supervisor is not None:
            # Only this invocation's child can be signalled during rollback.
            if os.name != "nt":
                setup.terminate_process(supervisor, timeout=30)
            else:
                stop_server()
                supervisor.wait(timeout=STOP_TIMEOUT)
        if not was_running:
            run(compose("stop", "--timeout", "30", "postgres"))
        raise


def down() -> None:
    validate_compose()
    stop_server()
    run(compose("stop", "--timeout", "30", "postgres"))
    print(
        "Local stack stopped. Database volume and configured media directories are preserved."
    )


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("up", "down", "_supervise"))
    args = parser.parse_args(argv)
    try:
        if args.command == "_supervise":
            supervise(dict(os.environ))
        else:
            with command_lock():
                (up if args.command == "up" else down)()
    except KeyboardInterrupt:
        print("Local stack command interrupted.", file=sys.stderr)
        return 130
    except (
        OSError,
        ValueError,
        StackError,
        setup.SetupError,
        subprocess.SubprocessError,
    ) as error:
        print(f"Local stack error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
