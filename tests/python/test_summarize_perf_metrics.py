"""Checks for bounded, reset-safe performance snapshot summaries."""
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("summarize_perf_metrics", ROOT / "scripts" / "summarize_perf_metrics.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
TOKEN = "00000000-0000-4000-8000-000000000001"


def event(counts, total, timestamp="2026-01-01T00:00:00Z"):
    fields = {"metric": "inspection_claim_acquire", "process_token": TOKEN,
              "count": sum(counts), "sum_micros": total}
    fields.update({name: count for (name, _), count in zip(MODULE.BUCKETS, counts, strict=True)})
    return json.dumps({"target": "melodee::performance", "timestamp": timestamp, "fields": fields}) + "\n"


class MetricsSummaryTests(unittest.TestCase):
    def test_cumulative_deltas_and_quantile_bounds(self):
        first = [1, 0, 0, 0, 0, 0, 0, 0, 0]
        last = [2, 2, 0, 0, 0, 0, 0, 0, 0]
        rows = MODULE.summarize(io.StringIO(event(first, 500) + event(last, 4500)))
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["observations"], 3)
        self.assertEqual(rows[0]["sum_micros"], 4000)
        self.assertEqual(rows[0]["p50_upper_ms"], 5)
        self.assertEqual(rows[0]["p99_upper_ms"], 5)

    def test_reset_and_malformed_counts_fail_closed(self):
        first = [1, 0, 0, 0, 0, 0, 0, 0, 0]
        empty = [0] * len(MODULE.BUCKETS)
        with self.assertRaises(ValueError):
            MODULE.summarize(io.StringIO(event(first, 1) + event(empty, 0)))
        with self.assertRaises(ValueError):
            MODULE.summarize(io.StringIO(event(first, 1).replace('"count": 1', '"count": 2')))

    def test_help_does_not_open_input(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts" / "summarize_perf_metrics.py"), "--help"],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0)
        self.assertIn("log", result.stdout)


if __name__ == "__main__":
    unittest.main()
