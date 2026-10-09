from __future__ import annotations

import copy
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "dev_stack.py"
with mock.patch.object(sys, "path", [str(ROOT / "scripts"), *sys.path]):
    spec = importlib.util.spec_from_file_location("dev_stack", SCRIPT)
    stack = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(stack)


class DevStackTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.runtime = self.root / ".local-dev"
        self.runtime.mkdir()
        for name, value in {
            "ROOT": self.root,
            "RUNTIME": self.runtime,
            "STATE": self.runtime / "server.json",
            "LOG": self.runtime / "server.log",
        }.items():
            patch = mock.patch.object(stack, name, value)
            patch.start()
            self.addCleanup(patch.stop)

    def state(self):
        return {"root": str(self.root), "port": 43210, "token": "a" * 64}

    def test_help_never_loads_environment_or_contacts_services(self):
        for arguments in (("--help",), ("up", "--help"), ("down", "--help")):
            result = subprocess.run(
                [sys.executable, "-S", str(SCRIPT), *arguments],
                cwd=self.root,
                env={"PATH": os.environ["PATH"]},
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("without resetting data", " ".join(result.stdout.split()))
        self.assertEqual(list(self.runtime.iterdir()), [])

    def test_environment_preserves_export_precedence_and_excludes_bootstrap_secret(
        self,
    ):
        (self.root / ".env").write_text(
            f"{stack.setup.DATABASE_ENV}={stack.setup.SAFE_DATABASE_URL}\n"
            "MELODEE__SERVER__BIND=127.0.0.1:8080\n"
        )
        with mock.patch.object(stack, "run", return_value="{}"), mock.patch.dict(
            os.environ,
            {
                "MELODEE__SERVER__BIND": "127.0.0.1:8081",
                stack.setup.BOOTSTRAP_PASSWORD_ENV: "do-not-inherit",
            },
            clear=True,
        ):
            environment = stack.load_environment()
        self.assertEqual(environment["MELODEE__SERVER__BIND"], "127.0.0.1:8081")
        self.assertNotIn(stack.setup.BOOTSTRAP_PASSWORD_ENV, environment)

    def test_compose_cors_default_respects_env_file_exports_and_empty_override(self):
        default = "https://feishin.vercel.app"
        for file_value, exported, expected in (
            (None, None, default),
            ("https://file.example", None, "https://file.example"),
            ("https://file.example", "https://export.example", "https://export.example"),
            ("", None, ""),
            (default, "", ""),
        ):
            with self.subTest(file_value=file_value, exported=exported):
                contents = f"{stack.setup.DATABASE_ENV}={stack.setup.SAFE_DATABASE_URL}\n"
                if file_value is not None:
                    contents += f"{stack.CORS_ENV}={file_value}\n"
                (self.root / ".env").write_text(contents)
                config = {"x-melodee-server-environment": {stack.CORS_ENV: default}}
                exports = {} if exported is None else {stack.CORS_ENV: exported}
                with (
                    mock.patch.dict(os.environ, exports, clear=True),
                    mock.patch.object(stack, "run", return_value=json.dumps(config)),
                ):
                    self.assertEqual(stack.load_environment()[stack.CORS_ENV], expected)

    def test_compose_defaults_reject_unrelated_environment_and_invalid_types(self):
        (self.root / ".env").write_text(
            f"{stack.setup.DATABASE_ENV}={stack.setup.SAFE_DATABASE_URL}\n"
        )
        for defaults in (None, [], {"PATH": "/other"}, {stack.CORS_ENV: None}):
            with (
                self.subTest(defaults=defaults),
                mock.patch.dict(os.environ, {}, clear=True),
                mock.patch.object(
                    stack, "run",
                    return_value=json.dumps({"x-melodee-server-environment": defaults}),
                ),
            ):
                with self.assertRaisesRegex(stack.StackError, "invalid x-melodee"):
                    stack.load_environment()

    def test_external_database_and_missing_env_fail_before_commands(self):
        with mock.patch.object(stack, "run") as run:
            self.assertEqual(stack.main(["up"]), 1)
            (self.root / ".env").write_text(
                "MELODEE__DATABASE__URL=postgres://external/example\n"
            )
            with mock.patch.dict(os.environ, {}, clear=True):
                self.assertEqual(stack.main(["up"]), 1)
        run.assert_not_called()

    def test_compose_drift_is_rejected_before_shutdown(self):
        config = {
            "name": "melodee-rs-dev",
            "services": {
                "postgres": {
                    "environment": {
                        "POSTGRES_DB": "melodee",
                        "POSTGRES_USER": "melodee",
                        "POSTGRES_PASSWORD": "melodee-dev",
                    },
                    "ports": [{"published": "5433", "target": 5432}],
                    "volumes": [
                        {
                            "type": "volume",
                            "source": "melodee_pg_data",
                            "target": "/var/lib/postgresql",
                        }
                    ],
                }
            },
            "volumes": {"melodee_pg_data": {"name": "melodee-rs-dev_melodee_pg_data"}},
        }
        with mock.patch.object(stack, "run", return_value=json.dumps(config)):
            stack.validate_compose()
        variants = []
        for section, key, value in (
            ("root", "name", "production"),
            ("services", "other", {}),
            ("postgres", "environment", {"POSTGRES_DB": "production"}),
            ("postgres", "ports", [{"published": "5432", "target": 5432}]),
            (
                "postgres",
                "volumes",
                [
                    {
                        "type": "bind",
                        "source": "/srv/postgres",
                        "target": "/var/lib/postgresql",
                    }
                ],
            ),
            ("volume", "external", True),
        ):
            changed = copy.deepcopy(config)
            target = {
                "root": changed,
                "services": changed["services"],
                "postgres": changed["services"]["postgres"],
                "volume": changed["volumes"]["melodee_pg_data"],
            }[section]
            target[key] = value
            variants.append(changed)
        for changed in variants:
            with (
                self.subTest(changed=changed),
                mock.patch.object(
                    stack, "run", return_value=json.dumps(changed)
                ) as run,
                mock.patch.object(stack, "stop_server") as stop,
            ):
                with self.assertRaises(stack.StackError):
                    stack.down()
                stop.assert_not_called()
                self.assertEqual(run.call_count, 1)

    def test_wrong_checkout_state_cannot_stop_anything(self):
        state = {**self.state(), "root": "/some/other/checkout"}
        stack.STATE.write_text(json.dumps(state))
        with mock.patch.object(stack, "control") as control:
            with self.assertRaises(stack.StackError):
                stack.stop_server()
        control.assert_not_called()

    def test_stale_state_is_removed_without_signalling_a_pid(self):
        stack.STATE.write_text(json.dumps(self.state()))
        with (
            mock.patch.object(stack, "control", return_value=None),
            mock.patch.object(os, "kill") as kill,
        ):
            stack.stop_server()
        kill.assert_not_called()
        self.assertFalse(stack.STATE.exists())

    def test_unverified_endpoint_never_receives_stop(self):
        stack.STATE.write_text(json.dumps(self.state()))
        with mock.patch.object(
            stack, "control", side_effect=stack.StackError("wrong identity")
        ) as control:
            with self.assertRaises(stack.StackError):
                stack.stop_server()
        self.assertEqual(control.call_args_list, [mock.call(self.state(), "status")])
        self.assertTrue(stack.STATE.exists())

    def test_up_waits_for_readiness_after_starting_postgres(self):
        with socket.socket() as available:
            available.bind(("127.0.0.1", 0))
            port = available.getsockname()[1]
        environment = {"MELODEE__SERVER__BIND": f"127.0.0.1:{port}"}
        supervisor = mock.Mock()
        supervisor.poll.return_value = None
        with (
            mock.patch.object(stack, "load_environment", return_value=environment),
            mock.patch.object(stack.shutil, "which", return_value="tool"),
            mock.patch.object(stack, "validate_compose"),
            mock.patch.object(stack, "wait_for_postgres"),
            mock.patch.object(stack, "run", return_value="") as run,
            mock.patch.object(stack.subprocess, "Popen", return_value=supervisor),
            mock.patch.object(
                stack, "read_state", side_effect=[None, self.state(), self.state()]
            ),
            mock.patch.object(stack, "control", return_value={"running": True}),
            mock.patch.object(stack, "ready", side_effect=[False, True]) as ready,
            mock.patch.object(stack.time, "sleep"),
        ):
            stack.up()
        self.assertEqual(ready.call_count, 2)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertIn(["pnpm", "install", "--frozen-lockfile"], commands)
        self.assertIn(["pnpm", "build"], commands)
        self.assertIn(
            stack.compose("up", "-d", "postgres"),
            commands,
        )
        self.assertNotIn(stack.compose("stop", "--timeout", "30", "postgres"), commands)

    def test_postgres_cold_start_waits_for_probe_success(self):
        with (
            mock.patch.object(
                stack.subprocess,
                "run",
                side_effect=[
                    mock.Mock(returncode=2),
                    mock.Mock(returncode=1),
                    mock.Mock(returncode=0),
                ],
            ) as run,
            mock.patch.object(stack.time, "sleep"),
        ):
            stack.wait_for_postgres()
        self.assertEqual(run.call_count, 3)
        for call in run.call_args_list:
            self.assertEqual(
                call.args[0],
                stack.compose(
                    "exec",
                    "-T",
                    "postgres",
                    "pg_isready",
                    "-U",
                    "melodee",
                    "-d",
                    "melodee",
                ),
            )

    def test_postgres_startup_wait_is_bounded(self):
        with (
            mock.patch.object(stack.time, "monotonic", side_effect=[0, 0, 601]),
            mock.patch.object(stack.time, "sleep"),
            mock.patch.object(
                stack.subprocess, "run", return_value=mock.Mock(returncode=2)
            ),
        ):
            with self.assertRaisesRegex(stack.StackError, "did not become ready"):
                stack.wait_for_postgres()

    def test_down_is_repeatable_without_env_and_only_stops_fixed_postgres(self):
        sentinel = self.root / "media"
        sentinel.write_bytes(b"preserve me")
        with (
            mock.patch.object(stack, "validate_compose"),
            mock.patch.object(stack, "run") as run,
        ):
            stack.down()
            stack.down()
        self.assertEqual(sentinel.read_bytes(), b"preserve me")
        self.assertEqual(
            [call.args[0] for call in run.call_args_list],
            [stack.compose("stop", "--timeout", "30", "postgres")] * 2,
        )

    def test_shutdown_failure_leaves_database_running(self):
        with (
            mock.patch.object(stack, "validate_compose"),
            mock.patch.object(
                stack, "stop_server", side_effect=stack.StackError("still running")
            ),
            mock.patch.object(stack, "run") as run,
        ):
            with self.assertRaises(stack.StackError):
                stack.down()
        run.assert_not_called()

    def test_repeat_up_does_not_build_or_start_another_server(self):
        stack.STATE.write_text(json.dumps(self.state()))
        with (
            mock.patch.object(stack, "load_environment", return_value={}),
            mock.patch.object(stack, "control", return_value={"running": True}),
            mock.patch.object(stack, "validate_compose"),
            mock.patch.object(stack, "wait_for_postgres") as wait,
            mock.patch.object(stack, "ready", return_value=True),
            mock.patch.object(stack, "run") as run,
        ):
            stack.up()
        run.assert_called_once_with(stack.compose("up", "-d", "postgres"))
        wait.assert_called_once_with()

    def test_occupied_server_port_is_never_killed_or_reused(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            environment = {
                "MELODEE__SERVER__BIND": f"127.0.0.1:{listener.getsockname()[1]}"
            }
            with (
                mock.patch.object(stack, "load_environment", return_value=environment),
                mock.patch.object(stack.shutil, "which", return_value="tool"),
                mock.patch.object(stack, "run") as run,
            ):
                with self.assertRaisesRegex(stack.StackError, "unavailable"):
                    stack.up()
                run.assert_not_called()

    def test_start_failure_rolls_back_only_postgres_started_by_this_command(self):
        for was_running in (False, True):
            with self.subTest(was_running=was_running), socket.socket() as available:
                available.bind(("127.0.0.1", 0))
                port = available.getsockname()[1]
            environment = {"MELODEE__SERVER__BIND": f"127.0.0.1:{port}"}
            commands = []

            def run(command, **_kwargs):
                commands.append(command)
                if "ps" in command:
                    return "postgres\n" if was_running else ""
                return ""

            with (
                mock.patch.object(stack, "load_environment", return_value=environment),
                mock.patch.object(stack.shutil, "which", return_value="tool"),
                mock.patch.object(stack, "validate_compose"),
                mock.patch.object(stack, "wait_for_postgres"),
                mock.patch.object(stack, "run", side_effect=run),
                mock.patch.object(
                    stack.subprocess, "Popen", side_effect=OSError("launch failed")
                ),
            ):
                with self.assertRaises(OSError):
                    stack.up()
            self.assertEqual(
                stack.compose("stop", "--timeout", "30", "postgres") in commands,
                not was_running,
            )

    def test_command_lock_is_exclusive_and_released_after_process_exit(self):
        code = (
            "import sys; from pathlib import Path; "
            f"sys.path.insert(0, {str(ROOT / 'scripts')!r}); import dev_stack as s; "
            "s.RUNTIME = Path(sys.argv[1]); "
            "\nwith s.command_lock(): pass\n"
        )
        with stack.command_lock():
            result = subprocess.run(
                [sys.executable, "-c", code, str(self.runtime)],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("another make up/down", result.stderr)
        with stack.command_lock():
            pass

    def test_real_supervisor_authenticates_stop_and_reaps_its_child(self):
        marker = self.root / "child-stopped"
        child = (
            "import signal, time; from pathlib import Path; "
            f"marker=Path({str(marker)!r}); "
            "signal.signal(signal.SIGTERM, lambda *_: (marker.write_text('stopped'), exit(0))); "
            "print('started', flush=True); time.sleep(60)"
        )
        code = (
            "import os, sys; from pathlib import Path; "
            f"sys.path.insert(0, {str(ROOT / 'scripts')!r}); import dev_stack as s; "
            "s.ROOT=Path(sys.argv[1]); s.RUNTIME=s.ROOT/'.local-dev'; s.STATE=s.RUNTIME/'server.json'; "
            "s.supervise(dict(os.environ), [sys.executable, '-c', sys.argv[2]])"
        )
        process = subprocess.Popen(
            [sys.executable, "-c", code, str(self.root), child],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.addCleanup(lambda: stack.setup.terminate_process(process))
        self.assertEqual(process.stdout.readline().strip(), "started")
        deadline = time.monotonic() + 5
        while not stack.STATE.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        state = stack.read_state()
        self.assertIsNotNone(state)
        # An unauthorized command cannot stop the real child.
        for token in ("b" * 64, "☃" * 64):
            with self.assertRaises((ValueError, OSError)):
                stack.control({**state, "token": token}, "stop")
        with socket.create_connection(
            ("127.0.0.1", state["port"]), timeout=2
        ) as connection:
            connection.sendall(b"[" * 1500 + b"]" * 1500 + b"\n")
            self.assertEqual(connection.recv(1), b"")
        self.assertTrue(stack.control(state, "status")["running"])
        stack.stop_server()
        output, errors = process.communicate(timeout=5)
        self.assertEqual(process.returncode, 0, output + errors)
        self.assertFalse(stack.STATE.exists())
        if os.name != "nt":
            self.assertEqual(marker.read_text(), "stopped")


if __name__ == "__main__":
    unittest.main()
