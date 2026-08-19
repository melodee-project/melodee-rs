"""Safety checks for the isolated Phase 11 rehearsal harness."""
from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts" / "legacy_rehearsal.py"
SPEC = importlib.util.spec_from_file_location("legacy_rehearsal", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class RehearsalInputTests(unittest.TestCase):
    def test_malformed_database_url_fails_without_echoing_credentials(self) -> None:
        with self.assertRaisesRegex(ValueError, "database URL") as context:
            MODULE.validate_database_url(
                "postgres://operator:super-secret@example.invalid"
            )
        self.assertNotIn("super-secret", str(context.exception))

    def test_synthetic_shape_is_bounded_before_database_creation(self) -> None:
        with self.assertRaisesRegex(ValueError, "between 1 and 10000000"):
            MODULE.validate_drill_config(0)
        with self.assertRaisesRegex(ValueError, "between 1 and 10000000"):
            MODULE.validate_drill_config(10_000_001)

    def test_missing_or_symlink_backup_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "does not exist"):
                MODULE.validate_backup_artifact(root / "missing.dump")

            source = root / "source.dump"
            source.write_bytes(b"not a postgres backup")
            with self.assertRaisesRegex(ValueError, "pg_restore"):
                MODULE.validate_backup_artifact(source)

            link = root / "link.dump"
            link.symlink_to(source)
            with self.assertRaisesRegex(ValueError, "regular"):
                MODULE.validate_backup_artifact(link)


if __name__ == "__main__":
    unittest.main()
