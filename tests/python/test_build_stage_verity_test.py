"""The sealed-tree build record is fail-closed and has safe help."""
from pathlib import Path
import subprocess
import contextlib
import io
import json
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import build_stage_verity_test as build


class SealedBuildTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/build_stage_verity_test.py'), '--help'],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_failed_compile_displays_cargo_diagnostics_and_mints_no_proof(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            proof = root / "proof"
            output = "\n".join([json.dumps({"reason": "compiler-message", "message": {"rendered": "error: moved value\n"}}),
                                 json.dumps({"reason": "other", "message": {"rendered": "do not show"}})])
            error = subprocess.CalledProcessError(101, ["cargo"], output=output)
            stderr = io.StringIO()
            with mock.patch.object(build, "command_output", return_value="head"), \
                 mock.patch.object(build, "source_fingerprints", return_value={}), \
                 mock.patch.object(build, "file_digest", return_value="digest"), \
                 mock.patch.object(build, "capture_cargo", side_effect=error) as child:
                with contextlib.redirect_stderr(stderr), self.assertRaises(subprocess.CalledProcessError):
                    build.build(root, proof)
            args, _ = child.call_args
            self.assertIn("--message-format=json-render-diagnostics", args[0])
            self.assertIn("error: moved value", stderr.getvalue())
            self.assertNotIn("do not show", stderr.getvalue())
            self.assertFalse(proof.exists())

    def test_capture_is_bounded_and_terminates_owned_noisy_child(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "exceeded limit"):
                build.capture_cargo([sys.executable, "-c", "import sys; sys.stdout.write('x'*100000); sys.stdout.flush()"],
                                    Path(directory), {}, limit=1024, timeout=10)

    def test_existing_output_is_never_overwritten_or_built(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(build, 'capture_cargo') as child:
            root = Path(directory)
            proof = root / 'proof'
            proof.write_text('preserve')
            with self.assertRaises(ValueError):
                build.build(root, proof)
            child.assert_not_called()
            self.assertEqual(proof.read_text(), 'preserve')


if __name__ == '__main__':
    unittest.main()
