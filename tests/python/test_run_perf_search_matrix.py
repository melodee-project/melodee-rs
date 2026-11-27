"""Bounds and ownership checks for search lifecycle benchmark collection."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import run_perf_search_matrix as matrix


class SearchMatrixTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/run_perf_search_matrix.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_unowned_target_fails_before_execution(self):
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(matrix.subprocess, "Popen") as process:
            with self.assertRaises(ValueError):
                matrix.run({}, Path(directory) / "perf_outbox-fake", Path(directory) / "out", 60, Path(directory) / "build.json")
            process.assert_not_called()

    def test_parser_keeps_only_fixed_bounded_fields(self):
        lines = []
        sizes = [16, 64, 256, 512, 512, 256, 64, 16]
        for trial, size in enumerate(sizes, 1):
            lines.append(f"BEGIN_INCREMENTAL_OUTBOX_MEASUREMENT trial={trial}")
            lines.append("DEBUG search index opened elapsed_seconds=0.01 writable=true")
            lines.append("DEBUG search index opened elapsed_seconds=0.02 writable=false")
            lines.append("DEBUG search writer initialized elapsed_seconds=0.123 private=discard")
            lines.append("DEBUG search writer committed commit_seconds=0.2 reload_seconds=0.1 pending_revisions=16 segments=4")
            lines.append(json.dumps({"workload": "postgres-search-incremental", "trial": trial,
                                     "batch_size": size, "batches": 1}, separators=(",", ":")))
        parsed = matrix.parse_log("\n".join(lines))
        self.assertEqual(len(parsed), 8)
        self.assertEqual(parsed[0]["lifecycle"]["writer_init"], [0.123])
        self.assertEqual(parsed[0]["lifecycle"]["index_open_writable"], [0.01])
        self.assertEqual(parsed[0]["lifecycle"]["index_open_readonly"], [0.02])
        self.assertEqual(parsed[0]["lifecycle"]["segments"], [4.0])
        self.assertNotIn("private", json.dumps(parsed))
        with self.assertRaises(ValueError):
            matrix.parse_log("\n".join(lines[:-1]))
        with self.assertRaises(ValueError):
            matrix.parse_log("\n".join(lines + [lines[0]]))


if __name__ == "__main__":
    unittest.main()
