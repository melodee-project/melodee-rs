"""Safety checks for release-server build provenance."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import build_perf_server as build


class BuildProvenanceTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/build_perf_server.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_existing_output_is_not_touched_and_no_build_occurs(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "record.json"
            output.write_text("untouched")
            with mock.patch.object(build.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    build.build(Path("/missing"), output)
            run.assert_not_called()
            self.assertEqual(output.read_text(), "untouched")

    def test_fingerprint_changes_with_source_and_rejects_symlinks(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name in ("apps", "crates"):
                (root / name).mkdir()
            for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
                (root / name).write_text(name)
            for name in build.STATIC_BUILD_INPUTS:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"embedded fixture")
            source = root / "apps/main.rs"
            source.write_text("one")
            first = build.source_fingerprints(root)
            source.write_text("two")
            self.assertNotEqual(first, build.source_fingerprints(root))
            link = root / "crates/link"
            try:
                link.symlink_to(source)
            except (OSError, NotImplementedError):
                self.skipTest("symlinks unavailable")
            with self.assertRaises(ValueError):
                build.source_fingerprints(root)


if __name__ == "__main__":
    unittest.main()
