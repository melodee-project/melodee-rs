"""Safety checks for the owned-fixture identity benchmark runner."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("run_perf_identity_benchmark", ROOT / "scripts/run_perf_identity_benchmark.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class IdentityBenchmarkRunnerTests(unittest.TestCase):
    def test_help_is_safe_without_fixture_or_database(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/run_perf_identity_benchmark.py"), "--help"],
                                env={"MELODEE__DATABASE__URL": "invalid"}, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0)
        self.assertIn("--test-binary", result.stdout)

    def test_binary_guard_requires_one_owned_release_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target/release/deps"
            target.mkdir(parents=True)
            one = target / "perf_identity_reconciliation-one"
            one.write_text("fixture", encoding="utf-8")
            one.chmod(0o755)
            outside = root / "perf_identity_reconciliation-outside"
            outside.write_text("fixture", encoding="utf-8")
            outside.chmod(0o755)
            with mock.patch.object(MODULE, "ROOT", root):
                self.assertEqual(MODULE.resolve_binary(None), one.resolve())
                self.assertEqual(MODULE.resolve_binary(one), one.resolve())
                with self.assertRaises(ValueError):
                    MODULE.resolve_binary(outside)
                two = target / "perf_identity_reconciliation-two"
                two.write_text("fixture", encoding="utf-8")
                two.chmod(0o755)
                with self.assertRaises(ValueError):
                    MODULE.resolve_binary(None)
                link = target / "perf_identity_reconciliation-link"
                link.symlink_to(outside)
                with self.assertRaises(ValueError):
                    MODULE.resolve_binary(link)


if __name__ == "__main__":
    unittest.main()
