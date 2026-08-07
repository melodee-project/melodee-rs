from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class RequirementExtractionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("extract_requirements", ROOT / "scripts" / "extract_requirements.py")

    def test_prd_has_large_unique_requirement_inventory(self) -> None:
        requirements = self.module.extract()
        ids = [item["id"] for item in requirements]
        self.assertGreaterEqual(len(ids), 200)
        self.assertEqual(len(ids), len(set(ids)))
        self.assertNotIn("unmapped", {item["phase"] for item in requirements})


class CaptureSafetyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("capture_baseline", ROOT / "scripts" / "capture_baseline.py")

    def test_sensitive_query_values_are_redacted(self) -> None:
        source = "https://example.test/rest/ping.view?u=alice&p=secret&t=hash&s=salt&c=test"
        redacted = self.module.redact_url(source)
        self.assertIn("u=alice", redacted)
        self.assertNotIn("secret", redacted)
        self.assertNotIn("hash", redacted)
        self.assertNotIn("salt", redacted)

    def test_public_scenarios_are_valid_json_array(self) -> None:
        value = json.loads((ROOT / "compatibility" / "scenarios" / "public.json").read_text())
        self.assertIsInstance(value, list)
        self.assertGreater(len(value), 0)
        self.assertTrue(all("name" in item and "path" in item for item in value))


class NewAdrTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("new_adr", ROOT / "scripts" / "new_adr.py")

    def test_slugify(self) -> None:
        self.assertEqual(self.module.slugify("Use PostgreSQL-backed leases"), "use-postgresql-backed-leases")


if __name__ == "__main__":
    unittest.main()
