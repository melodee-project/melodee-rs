"""Tests for the fail-closed current-client evidence schema."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts" / "validate_client_acceptance.py"
SPEC = importlib.util.spec_from_file_location("validate_client_acceptance", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ClientAcceptanceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.matrix = json.loads(
            (ROOT / "compatibility" / "clients" / "r4" / "acceptance-matrix.json").read_text(
                encoding="utf-8"
            )
        )

    def test_committed_pending_matrix_is_valid(self) -> None:
        counts = MODULE.validate_matrix(self.matrix)
        self.assertEqual(counts["pending-external"], 5)
        self.assertEqual(counts["pass"], 0)

    def test_passing_row_requires_provenance(self) -> None:
        row = self.matrix["rows"][0]
        row.update({"status": "pass", "result": "pass"})
        with self.assertRaisesRegex(ValueError, "serverCommit"):
            MODULE.validate_matrix(self.matrix)

    def test_passing_row_rejects_absolute_evidence_path(self) -> None:
        row = self.matrix["rows"][0]
        row.update(
            {
                "status": "pass",
                "result": "pass",
                "serverCommit": "a" * 40,
                "migratedRunId": "client-run-20260819",
                "clientVersion": "1.0.0",
                "platformVersion": "test-platform",
                "configurationFingerprint": "b" * 16,
                "command": "scripts/run-client-smoke.sh",
                "evidence": ["/tmp/client-report.json"],
            }
        )
        with self.assertRaisesRegex(ValueError, "repository-relative"):
            MODULE.validate_matrix(self.matrix)

    def test_passing_row_rejects_missing_evidence_file(self) -> None:
        row = self.matrix["rows"][0]
        row.update(
            {
                "status": "pass",
                "result": "pass",
                "serverCommit": "a" * 40,
                "migratedRunId": "client-run-20260819",
                "clientVersion": "1.0.0",
                "platformVersion": "test-platform",
                "configurationFingerprint": "b" * 16,
                "command": "scripts/run-client-smoke.sh",
                "evidence": ["design/evidence/r4/does-not-exist.json"],
            }
        )
        with self.assertRaisesRegex(ValueError, "existing repository file"):
            MODULE.validate_matrix(self.matrix)

    def _complete_result_row(self, row: dict[str, object], *, status: str = "pass") -> None:
        identity = (row["client"], row["surface"])
        workflow_status = "pass" if status == "pass" else "fail"
        row.update(
            {
                "status": status,
                "result": status,
                "serverCommit": "a" * 40,
                "migratedRunId": "r4-run-20260819",
                "clientVersion": "1.0.0",
                "platformVersion": "test-platform",
                "configurationFingerprint": "b" * 16,
                "command": "scripts/run-client-smoke.sh",
                "evidence": ["design/evidence/r4/README.md"],
                "workflowResults": {
                    workflow_id: {
                        "status": workflow_status,
                        **({"notes": "workflow failed during external acceptance"} if status == "fail" else {}),
                    }
                    for workflow_id in MODULE.REQUIRED_WORKFLOWS[identity]
                },
            }
        )

    def test_unredacted_secret_is_rejected(self) -> None:
        self.matrix["rows"][0]["operatorNote"] = "password=do-not-commit"
        with self.assertRaisesRegex(ValueError, "unredacted secret"):
            MODULE.validate_matrix(self.matrix)

    def test_reported_row_requires_every_named_workflow(self) -> None:
        row = self.matrix["rows"][0]
        self._complete_result_row(row)
        row["workflowResults"].pop("scrobble")
        with self.assertRaisesRegex(ValueError, "exactly the required workflow IDs"):
            MODULE.validate_matrix(self.matrix)

    def test_passing_row_rejects_failed_workflow(self) -> None:
        row = self.matrix["rows"][0]
        self._complete_result_row(row)
        row["workflowResults"]["scrobble"] = {
            "status": "fail",
            "notes": "client could not complete scrobble",
        }
        with self.assertRaisesRegex(ValueError, "must all pass"):
            MODULE.validate_matrix(self.matrix)

    def test_all_pass_status_requires_all_rows_with_matching_results(self) -> None:
        for row in self.matrix["rows"]:
            self._complete_result_row(row)
        self.matrix["status"] = "pass"
        counts = MODULE.validate_matrix(self.matrix)
        self.assertEqual(counts["pass"], 5)


if __name__ == "__main__":
    unittest.main()
