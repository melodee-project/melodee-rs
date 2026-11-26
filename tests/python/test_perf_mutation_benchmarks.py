"""Ownership and execution-scope checks for the performance workload runner."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "perf_mutation_benchmarks", ROOT / "scripts" / "run_perf_mutation_benchmarks.py"
)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
NAME = "melodee_template_" + "c" * 32
ENV = {"MELODEE_PERF_OWNED_DATABASE": NAME, "MELODEE__DATABASE__URL": f"postgres://localhost/{NAME}"}


class MutationBenchmarkTests(unittest.TestCase):
    def test_help_does_not_connect(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts" / "run_perf_mutation_benchmarks.py"), "--help"],
                                env={"MELODEE__DATABASE__URL": "invalid"}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_refuses_unowned_or_overridden_database(self):
        cases = [
            {**ENV, "MELODEE_PERF_OWNED_DATABASE": "postgres"},
            {**ENV, "MELODEE__DATABASE__URL": "postgres://localhost/postgres"},
            {**ENV, "MELODEE__DATABASE__URL": ENV["MELODEE__DATABASE__URL"] + "?dbname=postgres"},
            {**ENV, "MELODEE__DATABASE__URL": "file:///" + NAME},
        ]
        with mock.patch.object(MODULE.subprocess, "run") as child:
            for env in cases:
                with self.assertRaises(ValueError):
                    MODULE.run([1000], env)
            child.assert_not_called()

    def test_runs_recommendation_then_distinct_sizes_and_stops_on_failure(self):
        calls = []
        def run(args, **kwargs):
            calls.append((args, kwargs["env"]))
            return subprocess.CompletedProcess(args, 2 if len(calls) == 2 else 0)
        with mock.patch.object(MODULE.subprocess, "run", side_effect=run):
            self.assertEqual(MODULE.run([1000, 10000], ENV), 2)
        self.assertEqual(len(calls), 2)
        self.assertIn("perf_recommendation_contention", calls[0][0])
        self.assertIn("perf_large_release_merge", calls[1][0])
        self.assertEqual(calls[1][1]["MELODEE_PERF_MERGE_TRACKS"], "1000")
        with self.assertRaises(ValueError):
            MODULE.run([1000, 1000], ENV)


if __name__ == "__main__":
    unittest.main()
