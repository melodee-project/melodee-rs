"""Safety checks for the disposable media benchmark entry point."""
import contextlib
import importlib.util
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "perf_codec_matrix.py"
SPEC = importlib.util.spec_from_file_location("perf_codec_matrix", SCRIPT)
assert SPEC and SPEC.loader
benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark)


class CodecMatrixSafetyTests(unittest.TestCase):
    def assert_rejected_before_build(self, arguments):
        with mock.patch.object(sys, "argv", [str(SCRIPT), *arguments]), \
             mock.patch.object(benchmark, "build") as build, \
             mock.patch.object(benchmark.subprocess, "run") as run, \
             contextlib.redirect_stdout(io.StringIO()), \
             contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                benchmark.main()
            build.assert_not_called()
            run.assert_not_called()

    def test_help_never_builds_or_launches_subprocesses(self):
        self.assert_rejected_before_build(["--help"])

    def test_existing_evidence_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "result.json"
            output.write_text("preserve me", encoding="utf-8")
            self.assert_rejected_before_build(["--output", str(output)])
            self.assertEqual(output.read_text(encoding="utf-8"), "preserve me")

    def test_invalid_worker_count_does_not_launch_work(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "result.json"
            self.assert_rejected_before_build(["--output", str(output), "--workers", "4", "--releases", "3"])
            self.assertFalse(output.exists())

    def test_unbounded_corpus_is_rejected_before_launch(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "result.json"
            self.assert_rejected_before_build(["--output", str(output), "--seconds", "601"])
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
