"""Bounded, path-free aggregation of opt-in media operation events."""
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import summarize_media_operations as perf


def event(**overrides):
    fields = {"message": "media operation completed", "operation": "media_file_hash",
              "process_id": 42, "process_token": "12345678-1234-4234-8234-123456789abc",
              "success": True, "complete_bytes": True, "read_bytes": 100,
              "written_bytes": 0, "elapsed_micros": 17}
    fields.update(overrides)
    return json.dumps({"target": "melodee::media_operations", "fields": fields}) + "\n"


class MediaOperationSummaryTests(unittest.TestCase):
    def test_help_is_safe_without_services(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/summarize_media_operations.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_counts_success_failure_and_partial_bytes_without_leaking_unrelated_fields(self):
        source = event() + event(success=False, complete_bytes=False, read_bytes=7,
                                 path="/private/secret")
        result = perf.summarize(io.StringIO(source))
        self.assertTrue(result["complete"])
        self.assertEqual(result["events"], 2)
        self.assertEqual(result["operations"]["media_file_hash"],
                         {"operations": 2, "succeeded": 1, "read_bytes": 107,
                          "written_bytes": 0, "incomplete_byte_counts": 1})
        self.assertNotIn("/private/secret", json.dumps(result))

    def test_unknown_labels_bad_values_and_truncated_lines_are_incomplete(self):
        for source in (event(operation="attacker-controlled"), event(read_bytes=-1),
                       event(process_token="invalid"), event()[:-1], "{bad json}\n"):
            with self.subTest(source=source):
                self.assertFalse(perf.summarize(io.StringIO(source))["complete"])

    def test_empty_and_bounded_inputs_never_claim_completeness(self):
        self.assertFalse(perf.summarize(io.StringIO(""))["complete"])
        with mock.patch.object(perf, "MAX_EVENTS", 1):
            result = perf.summarize(io.StringIO(event() + event()))
        self.assertTrue(result["input_limited_or_truncated"])
        self.assertFalse(result["complete"])


if __name__ == "__main__":
    unittest.main()
