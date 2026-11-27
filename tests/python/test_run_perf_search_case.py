"""Scope and timeout checks for the disposable search benchmark wrapper."""
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "run_perf_search_case", ROOT / "scripts" / "run_perf_search_case.py"
)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class SearchCaseTests(unittest.TestCase):
    def test_help_is_safe_without_database(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts/run_perf_search_case.py"), "--help"],
            env={}, capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("--timeout-seconds", result.stdout)

    def test_missing_marker_rejects_before_starting_child(self):
        with mock.patch.object(sys, "argv", ["case", "--test-binary", "perf_rebuild-test"]), \
             mock.patch.dict(os.environ, {}, clear=True), \
             mock.patch.object(MODULE.subprocess, "run") as run, \
             contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(MODULE.main(), 1)
            run.assert_not_called()

    def test_timeout_cleans_only_the_owned_temp_tree(self):
        with tempfile.TemporaryDirectory() as outer:
            binary = Path(outer) / "perf_rebuild-test"
            binary.touch()
            unrelated = Path(outer) / "unrelated"
            unrelated.write_text("preserve")
            observed = []
            def timeout(command, *, env, check, timeout):
                private = Path(env["TMPDIR"])
                observed.append(private)
                (private / "index").write_text("owned")
                raise subprocess.TimeoutExpired(command, timeout)
            with mock.patch.object(sys, "argv", ["case", "--test-binary", str(binary), "--timeout-seconds", "1"]), \
                 mock.patch.dict(os.environ, {
                     "MELODEE_PERF_OWNED_DATABASE": "melodee_template_0123456789abcdef0123456789abcdef",
                     "MELODEE__DATABASE__URL": "fixture-only",
                 }, clear=True), \
                 mock.patch.object(MODULE.subprocess, "run", side_effect=timeout), \
                 contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(MODULE.main(), 124)
            self.assertEqual(len(observed), 1)
            self.assertFalse(observed[0].exists())
            self.assertEqual(unrelated.read_text(), "preserve")

    def test_concurrent_search_binary_is_allowed(self):
        with tempfile.TemporaryDirectory() as outer:
            binary = Path(outer) / "perf_outbox_concurrent-test"
            binary.touch()
            with mock.patch.object(sys, "argv", ["case", "--test-binary", str(binary)]), \
                 mock.patch.dict(os.environ, {
                     "MELODEE_PERF_OWNED_DATABASE": "melodee_template_0123456789abcdef0123456789abcdef",
                     "MELODEE__DATABASE__URL": "fixture-only",
                 }, clear=True), \
                 mock.patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
                self.assertEqual(MODULE.main(), 0)
                run.assert_called_once()


if __name__ == "__main__":
    unittest.main()
