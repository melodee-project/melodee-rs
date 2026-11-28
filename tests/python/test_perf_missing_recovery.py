"""Safety checks for the disposable missing-recovery workload."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_missing_recovery as benchmark  # noqa: E402


class OptionsTests(unittest.TestCase):
    def test_options_fail_closed(self) -> None:
        for items, timeout in ((0, 1), (35001, 1), (1, 0), (1, 1801)):
            with self.subTest(items=items, timeout=timeout):
                with self.assertRaises(ValueError):
                    benchmark.validate_options(items, timeout)


class SeedTests(unittest.TestCase):
    def test_seed_is_bounded_and_contains_only_owned_synthetic_items(self) -> None:
        statement = benchmark.seed_sql(7)
        self.assertIn("generate_series(1,7)", statement)
        self.assertIn("benchmark-missing-", statement)
        self.assertNotIn("DELETE", statement.upper())
        with self.assertRaises(ValueError):
            benchmark.seed_sql(35001)

    def test_retry_plan_uses_real_fenced_workload_predicate(self) -> None:
        plan = benchmark.RETRY_PLAN_SQL
        self.assertIn("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)", plan)
        self.assertIn("discovery_missing_retry_generation", plan)
        self.assertIn("inbound_active", plan)
        self.assertIn("lease_owner IS NULL", plan)
        for statement in ("INSERT", "UPDATE", "DELETE"):
            self.assertNotIn(statement, plan)


class OwnershipTests(unittest.TestCase):
    def test_unowned_database_rejected_before_access(self) -> None:
        with patch.object(benchmark.test_database, "sql") as database:
            with self.assertRaises(ValueError):
                benchmark.run(1, 1, Path("/missing"), Path("/missing"),
                              {"MELODEE__DATABASE__URL": "postgresql://localhost/production",
                               "MELODEE_PERF_OWNED_DATABASE": "production"}, None)
        database.assert_not_called()


class HelpTests(unittest.TestCase):
    def test_help_needs_no_database(self) -> None:
        env = {key: value for key, value in os.environ.items() if not key.startswith("MELODEE")}
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_missing_recovery.py"), "--help"],
                                cwd=ROOT, env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0)
        self.assertIn("--provenance", result.stdout)


if __name__ == "__main__":
    unittest.main()
