"""Tests for the Phase 8 OpenSubsonic and Jellyfin route-evidence matrices.

The matrices join the frozen Phase 0 inventories with the Phase 8 evidence
registries.  These tests pin the generation rules: legacy capture is never an
implementation claim, evidence rows must reference real inventory entries, and
the committed matrices must not drift from their sources.
"""
from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

sys.path.insert(0, str(ROOT / "tests" / "python"))
from test_scripts import load_module  # noqa: E402

matrix_builder = load_module(
    "build_phase8_route_matrix", ROOT / "scripts" / "build_phase8_route_matrix.py"
)

OS_INVENTORY = ROOT / "compatibility" / "opensubsonic" / "contracts" / "route-inventory.json"
OS_EVIDENCE = ROOT / "compatibility" / "opensubsonic" / "contracts" / "phase8-route-evidence.json"
OS_MATRIX = ROOT / "design" / "agents" / "PHASE-8-OPENSUBSONIC-ROUTE-MATRIX.json"
JF_INVENTORY = ROOT / "compatibility" / "jellyfin" / "contracts" / "route-inventory.json"
JF_EVIDENCE = ROOT / "compatibility" / "jellyfin" / "contracts" / "phase8-route-evidence.json"
JF_MATRIX = ROOT / "design" / "agents" / "PHASE-8-JELLYFIN-ROUTE-MATRIX.json"


class OpenSubsonicMatrixTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.inventory = json.loads(OS_INVENTORY.read_text(encoding="utf-8"))
        cls.evidence = json.loads(OS_EVIDENCE.read_text(encoding="utf-8"))
        cls.matrix = json.loads(OS_MATRIX.read_text(encoding="utf-8"))

    def test_matrix_is_generated_from_inventory_and_rust_evidence(self) -> None:
        generated = matrix_builder.build_opensubsonic_matrix(self.inventory, self.evidence)
        self.assertEqual(self.matrix, generated)
        self.assertEqual(self.matrix["endpointCount"], 81)
        self.assertEqual(
            self.matrix["pendingEndpointCount"]
            + self.matrix["completedEndpointCount"]
            + self.matrix["deferredEndpointCount"],
            81,
        )

    def test_legacy_capture_is_not_a_rust_implementation_claim(self) -> None:
        generated = matrix_builder.build_opensubsonic_matrix(
            self.inventory, {**self.evidence, "endpoints": []}
        )
        ping = next(row for row in generated["endpoints"] if row["endpoint"] == "ping")
        self.assertEqual(ping["phase8"]["status"], "implementation-pending")
        self.assertGreater(len(ping["legacyContract"]["scenarios"]), 0)

    def test_gone_and_not_implemented_endpoints_are_negative_obligations(self) -> None:
        generated = matrix_builder.build_opensubsonic_matrix(
            self.inventory, {**self.evidence, "endpoints": []}
        )
        by_name = {row["endpoint"]: row for row in generated["endpoints"]}
        self.assertEqual(
            by_name["getChatMessages"]["phase8"]["status"], "negative-compatibility-pending"
        )
        self.assertEqual(
            by_name["changePassword"]["phase8"]["status"], "negative-compatibility-pending"
        )
        negative = [
            row
            for row in generated["endpoints"]
            if row["phase8"]["status"] == "negative-compatibility-pending"
        ]
        self.assertEqual(len(negative), 10)

    def test_evidence_rejects_unknown_endpoints(self) -> None:
        bogus = {
            **self.evidence,
            "endpoints": [
                {
                    "endpoint": "notARealEndpoint",
                    "status": "implemented-and-differentially-tested",
                    "authority": "design/adr/ADR-0102-preserve-public-apis-as-separate-adapters.md",
                    "requirements": ["PRD-API-OS-004"],
                    "legacyEvidence": ["x"],
                    "rustTests": [],
                }
            ],
        }
        with self.assertRaisesRegex(matrix_builder.MatrixError, "unknown endpoint"):
            matrix_builder.build_opensubsonic_matrix(self.inventory, bogus)

    def test_every_endpoint_keeps_its_legacy_classification(self) -> None:
        classifications = {
            row["endpoint"]: row["legacyContract"]["classification"]
            for row in self.matrix["endpoints"]
        }
        self.assertEqual(classifications["hls"], "gone")
        self.assertEqual(classifications["getUsers"], "not-implemented")
        self.assertEqual(classifications["jukeboxControl"], "conditional")
        self.assertEqual(classifications["ping"], "supported")


class JellyfinMatrixTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.inventory = json.loads(JF_INVENTORY.read_text(encoding="utf-8"))
        cls.evidence = json.loads(JF_EVIDENCE.read_text(encoding="utf-8"))
        cls.matrix = json.loads(JF_MATRIX.read_text(encoding="utf-8"))

    def test_matrix_is_generated_from_inventory_and_rust_evidence(self) -> None:
        generated = matrix_builder.build_jellyfin_matrix(self.inventory, self.evidence)
        self.assertEqual(self.matrix, generated)
        self.assertEqual(self.matrix["routeCount"], 60)
        self.assertEqual(
            self.matrix["pendingRouteCount"]
            + self.matrix["completedRouteCount"]
            + self.matrix["deferredRouteCount"],
            60,
        )

    def test_legacy_capture_is_not_a_rust_implementation_claim(self) -> None:
        generated = matrix_builder.build_jellyfin_matrix(
            self.inventory, {**self.evidence, "routes": []}
        )
        authenticate = next(
            row
            for row in generated["routes"]
            if row["method"] == "POST" and row["path"].endswith("AuthenticateByName")
        )
        self.assertEqual(authenticate["phase8"]["status"], "implementation-pending")
        self.assertGreater(len(authenticate["legacyContract"]["scenarios"]), 0)

    def test_evidence_rejects_unknown_routes(self) -> None:
        bogus = {
            **self.evidence,
            "routes": [
                {
                    "method": "GET",
                    "path": "/api/jf/NotARealRoute",
                    "status": "implemented-and-differentially-tested",
                    "authority": "design/adr/ADR-0102-preserve-public-apis-as-separate-adapters.md",
                    "requirements": ["PRD-API-JF-004"],
                    "legacyEvidence": ["x"],
                    "rustTests": [],
                }
            ],
        }
        with self.assertRaisesRegex(matrix_builder.MatrixError, "unknown route"):
            matrix_builder.build_jellyfin_matrix(self.inventory, bogus)

    def test_every_route_keeps_its_client_visible_path(self) -> None:
        for row in self.matrix["routes"]:
            self.assertIn("clientVisiblePath", row)
            self.assertTrue(row["path"].startswith("/api/jf"))


if __name__ == "__main__":
    unittest.main()
