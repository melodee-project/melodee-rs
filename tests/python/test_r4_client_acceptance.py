"""Tests for the historical R4 client acceptance validator wrapper."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts" / "validate_r4_client_acceptance.py"
SPEC = importlib.util.spec_from_file_location("validate_r4_client_acceptance", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class R4ClientAcceptanceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.matrix = json.loads(
            (ROOT / "compatibility" / "clients" / "r4" / "acceptance-matrix.json").read_text(
                encoding="utf-8"
            )
        )

    def test_wrapper_validates_the_committed_matrix(self) -> None:
        counts = MODULE.validate_matrix(self.matrix)
        self.assertEqual(counts["pending-external"], 5)
        self.assertEqual(counts["pass"], 0)

    def test_wrapper_points_at_the_committed_matrix(self) -> None:
        self.assertEqual(
            MODULE.DEFAULT_MATRIX,
            ROOT / "compatibility" / "clients" / "r4" / "acceptance-matrix.json",
        )


if __name__ == "__main__":
    unittest.main()
