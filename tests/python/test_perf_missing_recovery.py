"""Safety checks for the disposable missing-recovery workload."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_missing_recovery as benchmark  # noqa: E402


@pytest.mark.parametrize("items,timeout", [(0, 1), (35001, 1), (1, 0), (1, 1801)])
def test_options_fail_closed(items: int, timeout: int) -> None:
    with pytest.raises(ValueError):
        benchmark.validate_options(items, timeout)


def test_seed_is_bounded_and_contains_only_owned_synthetic_items() -> None:
    statement = benchmark.seed_sql(7)
    assert "generate_series(1,7)" in statement
    assert "benchmark-missing-" in statement
    assert "DELETE" not in statement.upper()
    with pytest.raises(ValueError):
        benchmark.seed_sql(35001)


def test_retry_plan_uses_real_fenced_workload_predicate() -> None:
    plan = benchmark.RETRY_PLAN_SQL
    assert "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)" in plan
    assert "discovery_missing_retry_generation" in plan
    assert "inbound_active" in plan and "lease_owner IS NULL" in plan
    assert "INSERT" not in plan and "UPDATE" not in plan and "DELETE" not in plan


def test_unowned_database_rejected_before_access() -> None:
    with patch.object(benchmark.test_database, "sql") as database:
        with pytest.raises(ValueError):
            benchmark.run(1, 1, Path("/missing"), Path("/missing"),
                          {"MELODEE__DATABASE__URL": "postgresql://localhost/production",
                           "MELODEE_PERF_OWNED_DATABASE": "production"}, None)
    database.assert_not_called()


def test_help_needs_no_database() -> None:
    env = {key: value for key, value in os.environ.items() if not key.startswith("MELODEE")}
    result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_missing_recovery.py"), "--help"],
                            cwd=ROOT, env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0
    assert "--provenance" in result.stdout
