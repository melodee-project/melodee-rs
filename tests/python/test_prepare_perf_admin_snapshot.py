"""Snapshot preparation only touches a new owned worktree."""
import json
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import prepare_perf_admin_snapshot as snapshot


class AdminSnapshotTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/prepare_perf_admin_snapshot.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_existing_directory_never_runs_git_or_overwrites(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(snapshot.subprocess, "run") as child:
            target = Path(directory) / "existing"
            target.mkdir()
            (target / "keep").write_text("preserve")
            with self.assertRaises(ValueError):
                snapshot.prepare(target)
            child.assert_not_called()
            self.assertEqual((target / "keep").read_text(), "preserve")

    def test_unregistered_worktree_is_never_mutated(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree = root / "tree"
            target = tree / snapshot.TARGET
            target.parent.mkdir(parents=True)
            target.write_text("keep")
            (root / "manifest.json").write_text(json.dumps({"version": 1}))
            (root / "candidate-admin-stats.rs").write_text("replacement")
            (root / "manifest.json").chmod(0o600)
            (root / "candidate-admin-stats.rs").chmod(0o600)
            with mock.patch.object(snapshot, "command_output", return_value="worktree /different"):
                with self.assertRaises(ValueError):
                    snapshot.restore(root)
            self.assertEqual(target.read_text(), "keep")

    def test_unprivate_snapshot_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.chmod(0o755)
            with self.assertRaises(ValueError):
                snapshot.restore(root)

    def test_symlinked_ancestor_cannot_redirect_restore(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "owned"
            root.mkdir(mode=0o700)
            tree, external = root / "tree", Path(directory) / "external"
            tree.mkdir()
            target = external / "melodee-server/src/admin_stats.rs"
            target.parent.mkdir(parents=True)
            target.write_bytes(b"external must survive")
            (tree / "apps").symlink_to(external, target_is_directory=True)
            saved = root / "candidate-admin-stats.rs"
            saved.write_bytes(b"replacement")
            saved.chmod(0o600)
            baseline = {snapshot.TARGET: hashlib.sha256(target.read_bytes()).hexdigest()}
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps({"version": 1, "head": "fixture", "baseline_sha256": baseline,
                "candidate_sha256": {}, "saved_candidate_sha256": snapshot.file_digest(saved)}))
            manifest.chmod(0o600)
            with mock.patch.object(snapshot, "command_output", side_effect=[f"worktree {tree.resolve()}", "fixture"]), \
                 mock.patch.object(snapshot, "source_fingerprints", return_value=baseline):
                with self.assertRaises((OSError, ValueError)):
                    snapshot.restore(root)
            self.assertEqual(target.read_bytes(), b"external must survive")

    def test_symlink_components_in_checkout_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            external = root / "external"
            external.mkdir()
            (root / "compatibility").symlink_to(external, target_is_directory=True)
            with self.assertRaises(ValueError):
                snapshot.reject_symlink_components(root, "compatibility/example/file")

    def test_hardlinked_target_is_never_modified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree = root / "tree"
            target = tree / snapshot.TARGET
            target.parent.mkdir(parents=True)
            external = root / "external"
            external.write_bytes(b"preserve")
            os.link(external, target)
            with self.assertRaises(ValueError):
                snapshot.replace_owned_target(tree, hashlib.sha256(b"preserve").hexdigest(), b"new")
            self.assertEqual(external.read_bytes(), b"preserve")

    def test_owned_target_replace_checks_exact_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            tree = Path(directory) / "tree"
            target = tree / snapshot.TARGET
            target.parent.mkdir(parents=True)
            target.write_bytes(b"old")
            with self.assertRaises(ValueError):
                snapshot.replace_owned_target(tree, "wrong", b"new")
            self.assertEqual(target.read_bytes(), b"old")
            snapshot.replace_owned_target(tree, hashlib.sha256(b"old").hexdigest(), b"new")
            self.assertEqual(target.read_bytes(), b"new")

    def test_hardlink_created_after_validation_keeps_original_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree = root / "tree"
            target = tree / snapshot.TARGET
            target.parent.mkdir(parents=True)
            target.write_bytes(b"old")
            external = root / "external"
            real_replace = os.replace
            def interleaved(*args, **kwargs):
                os.link(target, external)
                return real_replace(*args, **kwargs)
            with mock.patch.object(snapshot.os, "replace", side_effect=interleaved):
                snapshot.replace_owned_target(tree, hashlib.sha256(b"old").hexdigest(), b"new")
            self.assertEqual(target.read_bytes(), b"new")
            self.assertEqual(external.read_bytes(), b"old")


if __name__ == "__main__":
    unittest.main()
