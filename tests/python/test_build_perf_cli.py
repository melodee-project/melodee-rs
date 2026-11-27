"""Migration CLI provenance build is bounded and refuses stale source evidence."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import os
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import build_perf_cli as cli


class MigrationBuildTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/build_perf_cli.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_existing_output_never_builds_or_overwrites(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(cli.subprocess, "run") as child:
            path = Path(directory) / "proof"
            path.write_text("keep")
            with self.assertRaises(ValueError):
                cli.build(Path(directory), path)
            child.assert_not_called()
            self.assertEqual(path.read_text(), "keep")

    def test_changed_source_rejects_build_proof(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target/release/melodee-cli"
            target.parent.mkdir(parents=True)
            target.write_bytes(b"cli")
            def command(args, _repo):
                return "true" if "--is-inside-work-tree" in args else "fixture"
            with mock.patch.object(cli, "command_output", side_effect=command), \
                 mock.patch.object(cli, "source_fingerprints", side_effect=[{"x": "before"}, {"x": "after"}]), \
                 mock.patch.object(cli.subprocess, "run") as child:
                with self.assertRaises(RuntimeError):
                    cli.build(root, root / "proof")
                child.assert_called_once()
            self.assertFalse((root / "proof").exists())

    def test_cross_target_fails_before_any_build(self):
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.dict(os.environ, {"CARGO_BUILD_TARGET": "aarch64-unknown-linux-gnu"}), \
             mock.patch.object(cli.subprocess, "run") as child:
            with self.assertRaises(ValueError):
                cli.build(Path(directory), Path(directory) / "proof")
            child.assert_not_called()


if __name__ == "__main__":
    unittest.main()
