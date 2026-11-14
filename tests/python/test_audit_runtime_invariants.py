"""Unit coverage for the runtime invariant audit (scripts/audit_runtime_invariants.py).

The audit is the layer that catches "the workflow never completes" and "the count
has no rows" failures that the mocked e2e suite and the API tests cannot see, so
the audit's own guard rails need proof: it must never mutate a database, its
checks must be unique and read-only, and a violation must fail the run.
"""

from __future__ import annotations

import importlib.util
import re
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "audit_runtime_invariants.py"


def load_audit_module():
    spec = importlib.util.spec_from_file_location("audit_runtime_invariants", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class AuditCheckInventoryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.audit = load_audit_module()

    def test_every_check_is_named_uniquely(self) -> None:
        names = [check.name for check in self.audit.CHECKS]
        self.assertEqual(len(names), len(set(names)))

    def test_every_check_documents_what_it_protects(self) -> None:
        for check in self.audit.CHECKS:
            self.assertGreater(
                len(check.protects.strip()),
                20,
                f"{check.name} must explain the invariant it protects",
            )

    def test_every_check_is_a_select(self) -> None:
        for check in self.audit.CHECKS:
            sql = check.sql.strip().lower()
            self.assertTrue(
                sql.startswith("select") or sql.startswith("with"),
                f"{check.name} must be a read-only statement",
            )
            if check.detail_sql:
                detail = check.detail_sql.strip().lower()
                self.assertTrue(detail.startswith("select") or detail.startswith("with"))

    def test_artist_status_dictionary_matches_the_domain(self) -> None:
        source = (ROOT / "crates/melodee-domain/src/artist.rs").read_text()
        body = source.split("pub enum ArtistStatus {", 1)[1].split("}", 1)[0]
        variants = re.findall(r"^\s*(\w+)(?:\([^)]*\))?,", body, re.MULTILINE)
        # MergedInto carries the target ID in the domain; its SQL discriminator is merged.
        expected = {"merged" if value == "MergedInto" else value.lower() for value in variants}
        check = next(item for item in self.audit.CHECKS if item.name == "catalog.artist_status_in_dictionary")
        self.assertEqual(set(re.findall(r"'([^']+)'", check.sql)), expected)

    def test_workflow_completion_and_drilldown_invariants_exist(self) -> None:
        # These two encode the classes of defect that reached the operator:
        # a Release parked in a non-terminal state, and a statistic whose rows
        # the UI could not list.
        names = {check.name for check in self.audit.CHECKS}
        self.assertIn("workflow.no_stuck_non_terminal", names)
        self.assertIn("workflow.indexed_requires_durable_projection", names)
        self.assertIn("catalog.artist_credits_backed_by_rows", names)
        self.assertIn("catalog.artist_type_in_dictionary", names)
        self.assertIn("catalog.current_overlay_artist_references", names)


class ReadOnlyGuardTests(unittest.TestCase):
    def setUp(self) -> None:
        self.audit = load_audit_module()

    def test_guard_rejects_mutating_statements(self) -> None:
        for statement in (
            "UPDATE artists SET status = 'verified'",
            "delete from tracks",
            "INSERT INTO artists (id) VALUES ($1)",
            "TRUNCATE release_work_items",
            "drop table artists",
            "ALTER TABLE artists ADD COLUMN x int",
        ):
            with self.assertRaises(self.audit.AuditError, msg=statement):
                self.audit.run_query("psql", "postgres://invalid", statement, 30)

    def test_guard_allows_identifiers_containing_keywords(self) -> None:
        # `updated_at`, `created_at`, and `last_error` must not trip the guard;
        # the guard is about statement kind, not substrings.
        for identifier in ("updated_at", "created_at", "last_error", "insert_count"):
            self.assertIsNone(
                self.audit.MUTATING_STATEMENT.search(f"SELECT {identifier} FROM t"),
                identifier,
            )


if __name__ == "__main__":
    unittest.main()
