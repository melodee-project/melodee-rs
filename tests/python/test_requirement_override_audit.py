import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "audit_requirement_overrides",
    ROOT / "scripts" / "audit_requirement_overrides.py",
)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class RequirementOverrideAuditTests(unittest.TestCase):
    def test_current_registry_has_complete_structural_coverage(self) -> None:
        result = MODULE.audit()
        self.assertEqual(result["total"], 239)
        self.assertEqual(result["override_count"], 239)
        self.assertEqual(
            result["status_counts"],
            {"deferred": 4, "implemented": 9, "in-progress": 12, "tested": 214},
        )
        self.assertEqual(len(result["nonterminal"]), 12)

    def test_strict_release_mode_rejects_current_nonterminal_requirements(self) -> None:
        with self.assertRaises(MODULE.OverrideAuditError) as context:
            MODULE.audit(require_terminal=True)
        self.assertIn("non-terminal requirements", str(context.exception))
        self.assertIn("PRD-MIG-007", str(context.exception))


if __name__ == "__main__":
    unittest.main()
