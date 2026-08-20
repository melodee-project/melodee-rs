"""Safety tests for the migrated-target real-client launcher."""
from __future__ import annotations

import importlib.util
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts" / "run_phase8_client_smoke.py"
SPEC = importlib.util.spec_from_file_location("run_phase8_client_smoke", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class Phase8ClientSmokeInputTests(unittest.TestCase):
    def test_loopback_port_allocator_returns_distinct_ephemeral_ports(self) -> None:
        ports = MODULE.allocate_loopback_ports(3)
        self.assertEqual(len(ports), len(set(ports)))
        self.assertTrue(all(1024 <= port <= 65535 for port in ports))

    def test_loopback_port_allocator_rejects_empty_request(self) -> None:
        with self.assertRaisesRegex(ValueError, "at least one"):
            MODULE.allocate_loopback_ports(0)

    def test_existing_target_accepts_postgres_url_without_printing_it(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            MODULE.validate_existing_target(
                "postgresql://operator:super-secret@example.test/migrated",
                root,
            )

    def test_existing_target_rejects_non_postgres_url(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "PostgreSQL URL") as context:
                MODULE.validate_existing_target("https://example.test/db", Path(directory))
            self.assertNotIn("example.test", str(context.exception))

    def test_existing_target_rejects_symlink_storage_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            target.mkdir()
            link = root / "link"
            link.symlink_to(target, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlink"):
                MODULE.validate_existing_target("postgres://operator@example.test/db", link)

    def test_existing_target_rejects_symlinked_storage_parent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            target.mkdir()
            parent = root / "parent"
            parent.symlink_to(root, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlink"):
                MODULE.validate_existing_target(
                    "postgres://operator@example.test/db", parent / "target"
                )

    def test_existing_target_url_is_read_from_named_environment_variable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ,
                {"MELODEE_R4_TEST_DATABASE_URL": "postgres://operator@example.test/db"},
                clear=False,
            ):
                value = MODULE.existing_target_from_env(
                    "MELODEE_R4_TEST_DATABASE_URL", root
                )
            self.assertEqual(value, "postgres://operator@example.test/db")

    def test_missing_existing_target_environment_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(os.environ, {}, clear=True):
                with self.assertRaisesRegex(ValueError, "environment variable") as context:
                    MODULE.existing_target_from_env(
                        "MELODEE_R4_MISSING_DATABASE_URL", Path(directory)
                    )
                self.assertNotIn("password", str(context.exception).lower())


if __name__ == "__main__":
    unittest.main()
