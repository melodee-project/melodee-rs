"""Scope guards for the optional real SIGTERM cleanup regression."""
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import verify_perf_interrupt_cleanup as check


class InterruptCleanupTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/verify_perf_interrupt_cleanup.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_unowned_database_rejected_before_any_database_access(self):
        with mock.patch.object(check.test_database, "sql") as sql:
            with self.assertRaises(ValueError):
                check.run(Path("/missing"), Path("/missing"), {"MELODEE__DATABASE__URL": "postgres://host/production"})
        sql.assert_not_called()

    def test_topology_mismatch_rejected_before_any_database_access(self):
        name = "melodee_template_" + "a" * 32
        env = {"MELODEE_PERF_OWNED_DATABASE": name,
               "MELODEE__DATABASE__URL": "postgres://one/" + name,
               "MELODEE_TEST_DATABASE_URL": "postgres://other/postgres"}
        with mock.patch.object(check.test_database, "sql") as sql:
            with self.assertRaises(ValueError):
                check.run(Path("/missing"), Path("/missing"), env)
        sql.assert_not_called()

    def test_fixture_signal_requires_exact_parent_session_marker(self):
        name = "melodee_template_" + "a" * 32
        env = {"MELODEE_PERF_OWNED_DATABASE": name,
               "MELODEE__DATABASE__URL": "postgres://one/" + name,
               "MELODEE_TEST_DATABASE_URL": "postgres://one/postgres",
               "MELODEE_PERF_FIXTURE_PID": "0"}
        with mock.patch.object(check.test_database, "sql") as sql, \
             mock.patch.object(check.os, "kill") as kill:
            with self.assertRaises(ValueError):
                check.run(Path("/missing"), Path("/missing"), env, target="fixture")
        sql.assert_not_called()
        kill.assert_not_called()


if __name__ == "__main__":
    unittest.main()
