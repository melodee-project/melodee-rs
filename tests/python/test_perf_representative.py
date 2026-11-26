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
            calls.append((args, kwargs))
            return subprocess.CompletedProcess(args, 0)
        def drop(_admin, target, created):
            self.assertEqual(target, name)
            created.remove(target)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--artists", "1", "--releases", "1", "--tracks-per-release", "1", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE.test_database, "migrate"), \
             mock.patch.object(MODULE.test_database, "drop_owned", side_effect=drop) as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(MODULE.main(), 0)
        self.assertEqual(calls[-1][0], ["workload"])
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_OWNED_DATABASE"], name)
        cleanup.assert_called_once()

    def test_workload_failure_still_cleans_up_owned_database(self):
        name = "melodee_template_" + "b" * 32
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **_kwargs):
            return subprocess.CompletedProcess(args, 1 if args == ["workload"] else 0)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE.test_database, "migrate"), \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(MODULE.main(), 1)
        self.assertEqual(cleanup.call_args.args[1], name)


if __name__ == "__main__":
    unittest.main()
