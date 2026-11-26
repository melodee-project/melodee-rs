"""Safety checks for staging a benchmark in the original baseline."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "prepare_perf_search_baseline", ROOT / "scripts" / "prepare_perf_search_baseline.py"
)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class BaselinePreparationTests(unittest.TestCase):
    def test_help_does_not_inspect_a_worktree(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts/prepare_perf_search_baseline.py"), "--help"],
            cwd=ROOT, capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("--worktree", result.stdout)

    def test_existing_different_file_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture"
            path.write_bytes(b"preserve")
            with self.assertRaises(ValueError):
                MODULE.needs_file(path, b"different")
            self.assertEqual(path.read_bytes(), b"preserve")

    def test_manifest_update_is_idempotent_and_fails_closed(self):
        initial = "[package]\nname = 'test'\n" + MODULE.NEEDLE + "\n[lints]\n"
        updated = MODULE.prepared_manifest(initial)
        self.assertEqual(MODULE.prepared_manifest(updated), updated)
        self.assertEqual(updated.count(MODULE.DEPENDENCY), 1)
        with self.assertRaises(ValueError):
            MODULE.prepared_manifest("[package]\n")

    def test_symlink_target_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "outside"
            target.mkdir()
            try:
                (root / "scripts").symlink_to(target, target_is_directory=True)
            except (OSError, NotImplementedError):
                self.skipTest("directory symlinks unavailable")
            with self.assertRaises(ValueError):
                MODULE.safe_worktree_path(root, "scripts/fixture.py")


if __name__ == "__main__":
    unittest.main()
