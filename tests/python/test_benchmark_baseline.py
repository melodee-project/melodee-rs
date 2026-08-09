"""Tests for the Issue 9 benchmark baseline artifacts."""
from __future__ import annotations

import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BENCH = ROOT / "compatibility" / "benchmarks"


class DatasetArtifactTests(unittest.TestCase):
    def test_all_tiers_generated(self) -> None:
        expected = {
            "tier-a.json": (1000, 1500, 15000),
            "tier-b.json": (35000, 50000, 560000),
            "tier-c.json": (100000, 500000, 5000000),
        }
        for name, counts in expected.items():
            path = BENCH / "datasets" / name
            self.assertTrue(path.exists(), name)
            data = json.loads(path.read_text())
            target = data["targetCounts"]
            self.assertEqual(
                (target["artists"], target["albums"], target["songs"]), counts, name
            )
            actual = tuple(int(x) for x in data["counts"].split("/"))
            self.assertEqual(actual, counts, f"{name} actual rows must match target")
            self.assertIn("songs", data["timingsSeconds"])

    def test_uuid_scheme_deterministic(self) -> None:
        for name in ("tier-a.json", "tier-b.json", "tier-c.json"):
            data = json.loads((BENCH / "datasets" / name).read_text())
            self.assertIn("md5('bench-", data["uuidScheme"])


class ResultArtifactTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.results = {
            p.stem.split("-")[1]: json.loads(p.read_text())
            for p in sorted((BENCH / "results").glob("tier-*.json"))
        }

    def test_all_tiers_have_results(self) -> None:
        self.assertEqual(set(self.results), {"a", "b", "c"})

    def test_environment_metadata_complete(self) -> None:
        required = [
            "dateUtc", "melodeeRsCommit", "legacyBaselineCommit", "tier", "database",
            "os", "architecture", "cpu", "logicalCpus", "memory", "postgres",
            "dotnet", "ffmpeg", "iterations", "command", "datasetSeed", "cacheState",
        ]
        for tier, result in self.results.items():
            for key in required:
                self.assertIn(key, result["environment"], f"{tier}:{key}")

    def test_matrix_covers_all_surfaces(self) -> None:
        for tier, result in self.results.items():
            surfaces = {r["surface"] for r in result["results"]}
            self.assertEqual(
                surfaces, {"melodee-api-v1", "opensubsonic", "jellyfin"}, tier
            )

    def test_all_endpoints_answered_200(self) -> None:
        for tier, result in self.results.items():
            for entry in result["results"]:
                self.assertEqual(entry["status"], 200, f"{tier}:{entry['id']}")

    def test_scale_cliffs_recorded(self) -> None:
        """The key scalability evidence must be present and ordered by tier."""
        random_songs = {
            tier: next(r for r in result["results"] if r["id"] == "os.getRandomSongs")
            for tier, result in self.results.items()
        }
        self.assertLess(
            random_songs["a"]["warmMedianMs"],
            random_songs["b"]["warmMedianMs"],
        )
        self.assertLess(
            random_songs["b"]["warmMedianMs"],
            random_songs["c"]["warmMedianMs"],
        )
        self.assertGreater(random_songs["c"]["warmMedianMs"], 60_000)
        memory = self.results["c"]["environment"].get("postBenchmarkServerMemory", "")
        self.assertIn("VmRSS", memory)

    def test_summary_md_exists(self) -> None:
        self.assertTrue((BENCH / "SUMMARY.md").exists())


if __name__ == "__main__":
    unittest.main()
