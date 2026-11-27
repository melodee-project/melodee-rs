"""Safety checks for the policy benchmark launcher."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
spec = importlib.util.spec_from_file_location("run_perf_policy_case", ROOT / "scripts" / "run_perf_policy_case.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class PolicyRunnerSafety(unittest.TestCase):
    def test_help_does_not_contact_database(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts" / "run_perf_policy_case.py"), "--help"],
                                env={"MELODEE__DATABASE__URL": "invalid"}, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)

    def test_outside_binary_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "perf_catalog_policy-test"
            binary.write_text("not executable")
            binary.chmod(0o700)
            with self.assertRaises(ValueError):
                module.resolve_binary(binary)

    def test_wrong_baseline_revision_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with patch.object(module.subprocess, "run") as run:
                run.side_effect = [
                    subprocess.CompletedProcess([], 0, stdout=str(root)),
                    subprocess.CompletedProcess([], 0, stdout="wrong-revision"),
                ]
                with self.assertRaises(ValueError):
                    module.resolve_binary(root / "nonexistent", root)

    def test_invalid_timeout_fails_before_database(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts" / "run_perf_policy_case.py"),
                                 "--test-binary", "/missing", "--timeout-seconds", "0"],
                                env={"MELODEE__DATABASE__URL": "invalid"}, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be positive", result.stderr)

    def test_timeout_is_reported_as_censored(self):
        with patch.object(module, "validate_fixture"), patch.object(
            module, "resolve_binary", return_value=Path("/validated/test")
        ), patch.object(module.subprocess, "run", side_effect=subprocess.TimeoutExpired("test", 2)), patch.object(
            sys, "argv", ["run_perf_policy_case.py", "--test-binary", "test", "--timeout-seconds", "2"]
        ):
            self.assertEqual(module.main(), 124)

if __name__ == "__main__":
    unittest.main()
