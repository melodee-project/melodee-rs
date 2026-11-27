"""Safety and size checks for the disposable representative fixture."""
import importlib.util
import contextlib
import io
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("perf_representative", ROOT / "scripts" / "perf_representative.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class RepresentativeFixtureTests(unittest.TestCase):
    def test_rejects_invalid_sizes(self):
        for sizes in [(0, 1, 1), (1, -1, 1), (1, 1, 0)]:
            with self.assertRaises(ValueError):
                MODULE.fixture_sql(*sizes)

    def test_help_is_safe_without_database(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "perf_representative.py"), "--help"],
            env={"MELODEE_TEST_DATABASE_URL": "invalid"}, capture_output=True, text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("--tracks-per-release", result.stdout)

    def test_workload_receives_exact_created_database_and_cleanup_runs(self):
        name = "melodee_template_" + "a" * 32
        calls = []
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **kwargs):
            if args[-1:] == ["migrate"]:
                self.assertEqual((kwargs["cwd"] / ".env").read_bytes(), b"")
                self.assertEqual(kwargs["env"]["MELODEE__STORAGE__ASSET_ROOT"], str(kwargs["cwd"]))
                self.assertNotIn("PRIVATE_TOKEN", kwargs["env"])
                self.assertNotIn("MELODEE__UNRELATED_SECRET", kwargs["env"])
            calls.append((args, kwargs))
            return subprocess.CompletedProcess(args, 0)
        def owned(args, **kwargs):
            calls.append((args, kwargs))
            return 0
        def drop(_admin, target, created):
            self.assertEqual(target, name)
            created.remove(target)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--artists", "1", "--releases", "1", "--tracks-per-release", "1", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE, "default_migration_cli", return_value=Path(sys.executable)), \
             mock.patch.dict(MODULE.os.environ, {"PRIVATE_TOKEN": "secret", "MELODEE__UNRELATED_SECRET": "secret"}), \
             mock.patch.object(MODULE.test_database, "drop_owned", side_effect=drop) as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE, "run_owned_child", side_effect=owned), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(MODULE.main(), 0)
        self.assertEqual(calls[-1][0], ["workload"])
        self.assertNotIn("PRIVATE_TOKEN", calls[-1][1]["env"])
        self.assertNotIn("MELODEE__UNRELATED_SECRET", calls[-1][1]["env"])
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_OWNED_DATABASE"], name)
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_FIXTURE_PID"], str(MODULE.os.getpid()))
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_FIXTURE_SHA256"], MODULE.file_digest(ROOT / "scripts/perf_representative.py"))
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_DATABASE_HELPER_SHA256"], MODULE.file_digest(ROOT / "scripts/test_database.py"))
        asset_root = Path(calls[-1][1]["env"]["MELODEE__STORAGE__ASSET_ROOT"])
        self.assertTrue(asset_root.is_absolute())
        self.assertFalse(asset_root.exists(), "invocation-owned assets must be cleaned")
        cleanup.assert_called_once()

    def test_workload_failure_still_cleans_up_owned_database(self):
        name = "melodee_template_" + "b" * 32
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **_kwargs):
            return subprocess.CompletedProcess(args, 0)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE, "default_migration_cli", return_value=Path(sys.executable)), \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE, "run_owned_child", return_value=1), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(MODULE.main(), 1)
        self.assertEqual(cleanup.call_args.args[1], name)

    def test_interrupted_workload_still_cleans_up_owned_database(self):
        name = "melodee_template_" + "e" * 32
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **_kwargs):
            return subprocess.CompletedProcess(args, 0)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE, "default_migration_cli", return_value=Path(sys.executable)), \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE, "run_owned_child", side_effect=InterruptedError("test")), \
             contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(MODULE.main(), 1)
        self.assertEqual(cleanup.call_args.args[1], name)

    def test_baseline_cli_targets_only_the_owned_database(self):
        name = "melodee_template_" + "c" * 32
        calls = []
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **kwargs):
            calls.append((args, kwargs))
            return subprocess.CompletedProcess(args, 0)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--migration-cli", sys.executable, "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE.test_database, "migrate") as default_migrate, \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE, "run_owned_child", return_value=0), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(MODULE.main(), 0)
        self.assertEqual(calls[0][0], [str(Path(sys.executable).resolve()), "migrate"])
        self.assertTrue(calls[0][1]["env"]["MELODEE__DATABASE__URL"].endswith("/" + name))
        default_migrate.assert_not_called()
        self.assertEqual(cleanup.call_args.args[1], name)

    def test_seed_timeout_is_censored_and_owned_database_is_cleaned(self):
        name = "melodee_template_" + "d" * 32
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **kwargs):
            if args[0] == "psql":
                raise subprocess.TimeoutExpired(args, kwargs["timeout"])
            return subprocess.CompletedProcess(args, 0)
        output = io.StringIO()
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--seed-timeout", "1", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE, "default_migration_cli", return_value=Path(sys.executable)), \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             contextlib.redirect_stdout(output):
            self.assertEqual(MODULE.main(), 124)
        self.assertIn('"status": "seed_timeout_censored"', output.getvalue())
        self.assertEqual(cleanup.call_args.args[1], name)

    @unittest.skipUnless(MODULE.os.name == "posix", "POSIX process groups required")
    def test_interrupted_owned_child_is_signalled_and_reaped(self):
        child = mock.Mock(pid=4567)
        child.wait.side_effect = [KeyboardInterrupt(), 0]
        child.poll.return_value = None
        with mock.patch.object(MODULE.subprocess, "Popen", return_value=child) as popen, \
             mock.patch.object(MODULE.os, "getpgid", return_value=4567), \
             mock.patch.object(MODULE.os, "getsid", return_value=4567), \
             mock.patch.object(MODULE.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                MODULE.run_owned_child(["workload"], ROOT, {})
        self.assertTrue(popen.call_args.kwargs["start_new_session"])
        kill.assert_called_once_with(4567, MODULE.signal.SIGTERM)
        self.assertEqual(child.wait.call_args.kwargs, {"timeout": 45})

    @unittest.skipUnless(MODULE.os.name == "posix", "POSIX process groups required")
    def test_changed_child_topology_is_never_signalled(self):
        child = mock.Mock(pid=4567)
        child.wait.side_effect = KeyboardInterrupt()
        child.poll.return_value = None
        with mock.patch.object(MODULE.subprocess, "Popen", return_value=child), \
             mock.patch.object(MODULE.os, "getpgid", return_value=99), \
             mock.patch.object(MODULE.os, "getsid", return_value=4567), \
             mock.patch.object(MODULE.os, "killpg") as kill:
            with self.assertRaises(RuntimeError):
                MODULE.run_owned_child(["workload"], ROOT, {})
        kill.assert_not_called()

    @unittest.skipUnless(MODULE.os.name == "posix", "POSIX process groups required")
    def test_unresponsive_owned_child_is_killed_only_after_topology_revalidation(self):
        child = mock.Mock(pid=4567)
        child.wait.side_effect = [KeyboardInterrupt(), subprocess.TimeoutExpired("workload", 45), 0]
        child.poll.return_value = None
        with mock.patch.object(MODULE.subprocess, "Popen", return_value=child), \
             mock.patch.object(MODULE.os, "getpgid", return_value=4567) as group, \
             mock.patch.object(MODULE.os, "getsid", return_value=4567), \
             mock.patch.object(MODULE.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                MODULE.run_owned_child(["workload"], ROOT, {})
        self.assertEqual(group.call_count, 2)
        self.assertEqual(kill.call_args_list, [mock.call(4567, MODULE.signal.SIGTERM),
                                               mock.call(4567, MODULE.signal.SIGKILL)])

    @unittest.skipUnless(MODULE.os.name == "posix", "POSIX process groups required")
    def test_topology_change_after_termination_refuses_group_kill(self):
        child = mock.Mock(pid=4567)
        child.wait.side_effect = [KeyboardInterrupt(), subprocess.TimeoutExpired("workload", 45)]
        child.poll.return_value = None
        with mock.patch.object(MODULE.subprocess, "Popen", return_value=child), \
             mock.patch.object(MODULE.os, "getpgid", side_effect=[4567, 99]), \
             mock.patch.object(MODULE.os, "getsid", return_value=4567), \
             mock.patch.object(MODULE.os, "killpg") as kill:
            with self.assertRaises(RuntimeError):
                MODULE.run_owned_child(["workload"], ROOT, {})
        kill.assert_called_once_with(4567, MODULE.signal.SIGTERM)


if __name__ == "__main__":
    unittest.main()
