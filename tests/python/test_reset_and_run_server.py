from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "reset_and_run_server.py"


def load_module():
    spec = importlib.util.spec_from_file_location("reset_and_run_server", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class ResetAndRunServerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module()

    def test_help_is_safe_without_rich_or_environment_loading(self) -> None:
        result = subprocess.run(
            [sys.executable, "-S", str(SCRIPT), "--help"],
            cwd=ROOT,
            env={"PATH": os.environ["PATH"]},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("fixed compose.dev.yaml development", result.stdout)
        self.assertIn("run without .env", result.stdout)

    def test_missing_env_is_reported_before_ui_or_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            missing = Path(temporary_directory) / ".env"
            with (
                mock.patch.object(self.module, "ENV_FILE", missing),
                mock.patch.object(
                    self.module, "display_startup_error"
                ) as display_error,
                mock.patch.object(self.module, "load_ui") as load_ui,
                mock.patch.object(self.module, "run_setup") as run_setup,
            ):
                result = self.module.main([])
        self.assertEqual(result, 2)
        display_error.assert_called_once()
        self.assertIn("was not found", display_error.call_args.args[0])
        load_ui.assert_not_called()
        run_setup.assert_not_called()

    def test_environment_parser_handles_export_quotes_comments_and_empty_values(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            env_file = Path(temporary_directory) / ".env"
            env_file.write_text(
                "# local configuration\n"
                "export SIMPLE=value\n"
                "QUOTED='value with spaces'\n"
                'DOUBLE="another value"\n'
                "EMPTY=\n"
                "COMMENTED=value # trailing comment\n",
                encoding="utf-8",
            )
            environment = self.module.parse_environment_file(env_file, {"BASE": "kept"})
        self.assertEqual(environment["BASE"], "kept")
        self.assertEqual(environment["SIMPLE"], "value")
        self.assertEqual(environment["QUOTED"], "value with spaces")
        self.assertEqual(environment["DOUBLE"], "another value")
        self.assertEqual(environment["EMPTY"], "")
        self.assertEqual(environment["COMMENTED"], "value")

    def test_environment_parser_rejects_malformed_assignments_without_values_in_errors(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            env_file = Path(temporary_directory) / ".env"
            env_file.write_text("NOT AN ASSIGNMENT\n", encoding="utf-8")
            with self.assertRaisesRegex(self.module.SetupError, "line 1") as raised:
                self.module.parse_environment_file(env_file)
        self.assertNotIn("NOT AN ASSIGNMENT", str(raised.exception))

    def test_only_the_exact_fixed_development_database_is_accepted(self) -> None:
        valid = {self.module.DATABASE_ENV: self.module.SAFE_DATABASE_URL}
        self.module.validate_development_environment(valid)
        for unsafe in (
            "postgres://melodee:melodee-dev@127.0.0.1:5432/melodee",
            "postgres://melodee:secret@database.example.test:5432/melodee",
            "postgres://melodee:melodee-dev@127.0.0.1:5433/other",
        ):
            with self.subTest(unsafe=unsafe):
                with self.assertRaisesRegex(
                    self.module.SetupError, "refusing to reset"
                ):
                    self.module.validate_development_environment(
                        {self.module.DATABASE_ENV: unsafe}
                    )

    def test_destructive_work_is_delegated_to_the_scoped_reset_helper(self) -> None:
        self.assertEqual(
            self.module.reset_command(),
            [sys.executable, str(ROOT / "scripts" / "reset_dev_database.py")],
        )
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertNotIn('"dropdb"', source)
        self.assertNotIn('"createdb"', source)
        self.assertNotIn('"volume", "rm"', source)

    def test_admin_password_is_never_part_of_the_cli_arguments(self) -> None:
        credentials = self.module.AdminCredentials(
            username="admin",
            email="admin@example.test",
            password="not-in-process-arguments",
        )
        command = self.module.admin_bootstrap_command(Path("melodee-cli"), credentials)
        self.assertNotIn(credentials.password, command)
        self.assertNotIn(self.module.BOOTSTRAP_PASSWORD_ENV, command)
        self.assertEqual(
            command,
            [
                "melodee-cli",
                "users",
                "bootstrap-admin",
                "--username",
                "admin",
                "--email",
                "admin@example.test",
            ],
        )

    def test_admin_ui_uses_the_frozen_lockfile_before_build(self) -> None:
        self.assertEqual(
            self.module.admin_ui_commands(["corepack", "pnpm"]),
            [
                ["corepack", "pnpm", "install", "--frozen-lockfile"],
                ["corepack", "pnpm", "build"],
            ],
        )

    def test_command_cancellation_terminates_and_reaps_the_child(self) -> None:
        process = mock.MagicMock()
        process.communicate.side_effect = KeyboardInterrupt
        with (
            mock.patch.object(self.module.subprocess, "Popen", return_value=process),
            mock.patch.object(self.module, "terminate_process") as terminate_process,
        ):
            with self.assertRaises(KeyboardInterrupt):
                self.module.run_checked(
                    ["example-command"],
                    cwd=ROOT,
                    environment={"PATH": os.environ["PATH"]},
                    timeout=1,
                )
        terminate_process.assert_called_once_with(process)

    def test_bind_parser_maps_wildcard_addresses_to_loopback_for_probing(self) -> None:
        ipv4 = self.module.parse_bind_address(
            {self.module.SERVER_BIND_ENV: "0.0.0.0:8080"}
        )
        ipv6 = self.module.parse_bind_address(
            {self.module.SERVER_BIND_ENV: "[::]:9090"}
        )
        self.assertEqual((ipv4.probe_host, ipv4.port), ("127.0.0.1", 8080))
        self.assertEqual((ipv6.probe_host, ipv6.port), ("::1", 9090))

    def test_rich_dependency_is_exactly_pinned(self) -> None:
        requirements = (ROOT / "scripts" / "requirements.txt").read_text(
            encoding="utf-8"
        )
        self.assertIn("rich==14.3.2", requirements.splitlines())

    def test_setup_orchestrates_the_documented_sequence_and_scopes_the_password(
        self,
    ) -> None:
        credentials = self.module.AdminCredentials(
            username="admin",
            email="admin@example.test",
            password="environment-only-password",
        )
        bind = self.module.BindAddress(
            configured="127.0.0.1:8080",
            host="127.0.0.1",
            port=8080,
        )
        environment = {
            self.module.DATABASE_ENV: self.module.SAFE_DATABASE_URL,
            self.module.SERVER_BIND_ENV: bind.configured,
            "PATH": os.environ["PATH"],
        }
        ui = mock.MagicMock()
        ui.confirm_reset.return_value = True
        ui.prompt_credentials.return_value = credentials
        monitor = mock.MagicMock()
        monitor.process.poll.return_value = 0
        commands: list[list[str]] = []
        observed_password: list[str | None] = []

        def record_stage(*args, **_kwargs):
            commands.append(list(args[5]))
            return ""

        def record_admin(command, **kwargs):
            commands.append(list(command))
            observed_password.append(
                kwargs["environment"].get(self.module.BOOTSTRAP_PASSWORD_ENV)
            )
            return ""

        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary_root = Path(temporary_directory)
            server = temporary_root / "melodee-server"
            cli = temporary_root / "melodee-cli"
            server.touch()
            cli.touch()

            def fake_binary_path(name: str) -> Path:
                return server if name == "melodee-server" else cli

            with (
                mock.patch.object(
                    self.module,
                    "parse_environment_file",
                    return_value=environment.copy(),
                ),
                mock.patch.object(self.module, "validate_development_environment"),
                mock.patch.object(self.module, "parse_bind_address", return_value=bind),
                mock.patch.object(
                    self.module, "require_commands", return_value=["pnpm"]
                ),
                mock.patch.object(self.module, "ensure_server_port_available"),
                mock.patch.object(self.module, "run_stage", side_effect=record_stage),
                mock.patch.object(
                    self.module, "binary_path", side_effect=fake_binary_path
                ),
                mock.patch.object(self.module, "launch_server", return_value=monitor),
                mock.patch.object(self.module, "wait_for_server"),
                mock.patch.object(self.module, "run_checked", side_effect=record_admin),
                mock.patch.object(self.module, "follow_server", return_value=0),
                mock.patch.object(self.module, "stop_server") as stop_server,
            ):
                result = self.module.run_setup(ui, assume_yes=False)

        self.assertEqual(result, 0)
        self.assertEqual(
            commands[:4],
            [
                self.module.reset_command(),
                ["pnpm", "install", "--frozen-lockfile"],
                ["pnpm", "build"],
                self.module.rust_build_command(),
            ],
        )
        self.assertEqual(commands[4][1:3], ["users", "bootstrap-admin"])
        self.assertNotIn(credentials.password, commands[4])
        self.assertEqual(observed_password, [credentials.password])
        stop_server.assert_called_once_with(monitor)


if __name__ == "__main__":
    unittest.main()
