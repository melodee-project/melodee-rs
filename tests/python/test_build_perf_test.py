"""Safety tests for the interactive-test provenance builder."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import build_perf_test as build


class PerfTestBuildTests(unittest.TestCase):
    def test_help_has_no_build_or_mutation(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/build_perf_test.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_existing_output_fails_before_child_execution(self):
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(build, "command_output") as command, \
             mock.patch.object(build.subprocess, "run") as run:
            output = Path(directory) / "result"
            output.write_text("keep")
            with self.assertRaises(ValueError):
                build.build(ROOT, output)
            self.assertEqual(output.read_text(), "keep")
            command.assert_not_called()
            run.assert_not_called()

    def test_changed_source_or_head_refuses_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "test-binary"
            executable.write_bytes(b"test")
            artifact = '{"reason":"compiler-artifact","target":{"name":"perf_interactive_routes"},' \
                       '"profile":{"test":true},"executable":' + __import__("json").dumps(str(executable)) + '}'
            result = subprocess.CompletedProcess([], 0, artifact)
            def command(args, cwd):
                return "true" if "--is-inside-work-tree" in args else "abc"
            with mock.patch.object(build, "command_output", side_effect=command), \
                 mock.patch.object(build, "source_fingerprints", side_effect=[{"a": "1"}, {"a": "2"}]), \
                 mock.patch.object(build.subprocess, "run", return_value=result) as run:
                with self.assertRaisesRegex(RuntimeError, "changed"):
                    build.build(root, root / "out")
                self.assertFalse((root / "out").exists())
                self.assertIn("--no-run", run.call_args.args[0])
                self.assertEqual(run.call_args.kwargs["env"]["CARGO_BUILD_JOBS"], "2")
                self.assertEqual(run.call_args.kwargs["env"]["RUST_TEST_THREADS"], "1")


if __name__ == "__main__":
    unittest.main()
