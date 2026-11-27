"""Guard policy benchmark baseline staging."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
spec = importlib.util.spec_from_file_location("prepare_perf_policy_baseline", ROOT / "scripts" / "prepare_perf_policy_baseline.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class PolicyBaselinePreparation(unittest.TestCase):
    def test_help_does_not_inspect_worktree(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/prepare_perf_policy_baseline.py"), "--help"],
                                cwd=ROOT, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
    def test_manifest_idempotent_and_conflicts_fail_closed(self):
        initial = "[package]\nname='test'\n[lints]\nworkspace=true\n"
        result = module.updated_manifest(initial)
        self.assertEqual(module.updated_manifest(result), result)
        with self.assertRaises(ValueError):
            module.updated_manifest("[dev-dependencies]\nother='1'\n[lints]\n")
        with self.assertRaises(ValueError):
            module.updated_manifest("[package]\n")

if __name__ == "__main__":
    unittest.main()
