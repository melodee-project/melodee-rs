"""Admin-stats comparison is bounded, read-only and fixture-scoped."""
import json
import hashlib
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import run_perf_admin_stats_ab as bench
import build_perf_test
import perf_media_io_trace


def sample():
    rows = [f"admin_stats_ab_iteration={i} elapsed_ms=12.5 response_bytes=100" for i in range(9)]
    rows.extend(['admin_stats_ab_response={"library":{"artists":2,"releases":3,"tracks":4,"formats":[]},"processing":{},"listening":{},"database":{}}',
                 "admin_stats_ab_fixture artists=2 releases=3 tracks=4"])
    return "\n".join(rows)


class AdminStatsComparisonTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/run_perf_admin_stats_ab.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(build_perf_test.TARGETS["admin-stats"],
                         ("melodee-server", "perf_admin_stats_ab", False))

    def test_only_the_reviewed_route_test_is_invoked(self):
        self.assertEqual(bench.test_command(Path("test-binary")),
                         ["test-binary", "measure_read_only_admin_stats_route", "--exact",
                          "--ignored", "--nocapture", "--test-threads=1"])

    def test_source_pair_records_dirty_shared_context_and_rejects_extra_difference(self):
        old_bytes = b"old admin code"
        source = {bench.CHANGED_FILE: hashlib.sha256(old_bytes).hexdigest(), "shared.rs": "dirty-source-hash"}
        candidate = {**source, bench.CHANGED_FILE: "candidate-hash"}
        old_digest, new_digest = bench.source_pair(source, candidate, old_bytes)
        self.assertNotEqual(old_digest, new_digest)
        with self.assertRaises(ValueError):
            bench.source_pair(source, {**candidate, "shared.rs": "different"}, old_bytes)

    def test_percentiles_exclude_each_process_warmup_and_report_sample_count(self):
        runs = [{"variant": variant, "samples": [{"elapsed_ms": float(n)} for n in range(9)]}
                for variant in ("baseline", "candidate", "candidate", "baseline")]
        result = bench.summarize_samples(runs)
        self.assertEqual(result["baseline"], {"count": 16, "p50_ms": 4.0, "p95_ms": 8.0, "p99_ms": 8.0})
        self.assertEqual(result["candidate"], result["baseline"])

    def test_parser_requires_nine_ordered_samples_and_matching_library(self):
        data = bench.parse_output(sample())
        self.assertEqual(len(data["samples"]), 9)
        prefixed = sample().replace("admin_stats_ab_iteration=0", "test measure_read_only_admin_stats_route ... admin_stats_ab_iteration=0")
        self.assertEqual(len(bench.parse_output(prefixed)["samples"]), 9)
        for invalid in (sample().replace("admin_stats_ab_iteration=8", "admin_stats_ab_iteration=7"),
                        sample().replace('"tracks":4', '"tracks":5'),
                        sample().replace("response_bytes=100", "response_bytes=0", 1),
                        sample().replace("admin_stats_ab_iteration=1", "test measure_read_only_admin_stats_route ... admin_stats_ab_iteration=1"),
                        sample().replace('"processing":{}', '"generatedAt":"time","processing":{}'),
                        sample().replace("admin_stats_ab_response=", "admin_stats_ab_missing=")):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                bench.parse_output(invalid)

    def test_normalized_response_comparison_retains_object_order(self):
        original = bench.parse_output(sample())["response"]
        reversed_top = sample().replace('"library":{"artists":2,"releases":3,"tracks":4,"formats":[]},"processing":{}',
                                        '"processing":{},"library":{"artists":2,"releases":3,"tracks":4,"formats":[]}')
        reversed_nested = sample().replace('"artists":2,"releases":3', '"releases":3,"artists":2')
        self.assertEqual(json.loads(original), json.loads(bench.parse_output(reversed_top)["response"]))
        self.assertNotEqual(original, bench.parse_output(reversed_top)["response"])
        self.assertNotEqual(original, bench.parse_output(reversed_nested)["response"])

    def test_cross_process_comparison_masks_only_nonnegative_integer_size_in_place(self):
        left = '{"library":{},"database":{"totalBytes":100,"tables":[],"poolInUse":8},"processing":{}}'
        changed_size = left.replace('100', '200')
        self.assertEqual(bench.cross_process_response(left), bench.cross_process_response(changed_size))
        for changed in (left.replace('"poolInUse":8', '"poolInUse":7'),
                        left.replace('"totalBytes":100,"tables":[]', '"tables":[],"totalBytes":100'),
                        left.replace('"library":{},"database"', '"database"').replace(',"processing":{}', ',"library":{},"processing":{}')):
            with self.subTest(changed=changed):
                self.assertNotEqual(bench.cross_process_response(left), bench.cross_process_response(changed))
        for value in ('true', '-1', '1.5', 'null', '"100"'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                bench.cross_process_response(left.replace('100', value))

    def test_failure_diagnostic_does_not_disclose_unrecognized_values(self):
        self.assertEqual(bench.safe_failure_reason(ValueError("incomplete admin-stats output")),
                         "incomplete admin-stats output")
        self.assertIsNone(bench.safe_failure_reason(ValueError("secret /private/path")))
        self.assertIsNone(bench.safe_failure_reason(RuntimeError("incomplete admin-stats output")))
        self.assertEqual(bench.safe_failure_reason(RuntimeError("baseline/candidate normalized response results differ")),
                         "baseline/candidate normalized response results differ")
        self.assertEqual(bench.test_failure_reason("secret /private/path normalized stats result changed between samples"),
                         "admin-stats test failed: normalized response changed within process")
        self.assertEqual(bench.test_failure_reason("secret /private/path"), "admin-stats test failed: other")

    def test_failure_field_labels_are_allowlisted_and_do_not_expose_values(self):
        left = {"library": {"secret": "/private/a"}, "database": {"totalBytes": 10, "poolInUse": 1}}
        right = {"library": {"secret": "/private/b"}, "database": {"totalBytes": 12, "poolInUse": 1}}
        raw = "left: " + json.dumps(json.dumps(left)) + "\nright: " + json.dumps(json.dumps(right))
        self.assertEqual(bench.changed_response_fields(raw), ["library", "database.totalBytes"])
        self.assertEqual(bench.response_field_labels(left, right), ["library", "database.totalBytes"])
        self.assertEqual(bench.changed_response_fields("left: /private/path"), [])

    def test_quiescence_record_is_bounded_and_requires_three_stable_observations(self):
        value = {"mode": "vacuum_and_size_stability", "stable_observations": 3,
                 "observations": 4, "duration_seconds": 2.5}
        self.assertEqual(bench.parse_quiescence(json.dumps(value)), value)
        self.assertIsNone(bench.parse_quiescence(None))
        for invalid in ({**value, "stable_observations": 2}, {**value, "observations": 2},
                        {**value, "duration_seconds": float("nan")}, {**value, "extra": "/private"}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                bench.parse_quiescence(json.dumps(invalid))

    def test_invalid_fixture_never_spawns(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(bench.subprocess, "Popen") as child:
            root = Path(directory)
            with self.assertRaises((KeyError, ValueError)):
                bench.run({}, root / "old", root / "new", root / "old.json", root / "new.json",
                          root / "cli.json", root / "out", 20)
            child.assert_not_called()

    def test_wrong_binary_hash_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "target/release/deps"
            root.mkdir(parents=True)
            binary = root / "perf_admin_stats_ab-123"
            binary.write_bytes(b"fake")
            binary.chmod(0o700)
            with self.assertRaises(ValueError):
                bench.validate_binary(binary, {"test_executable_name": binary.name,
                                               "test_executable_sha256": "wrong"})

    @unittest.skipUnless(os.name == "posix" and hasattr(os, "pidfd_open") and hasattr(os, "P_PIDFD")
                         and hasattr(signal, "pidfd_send_signal"), "Linux pidfd required")
    def test_exited_unreaped_leader_pins_session_until_descendant_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "child"
            code = ("import pathlib,subprocess,sys; "
                    "p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); "
                    "pathlib.Path(sys.argv[1]).write_text(str(p.pid))")
            child = subprocess.Popen([sys.executable, "-c", code, str(pid_file)],
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL, start_new_session=True)
            leader = os.pidfd_open(child.pid)
            try:
                deadline = time.monotonic() + 10
                while os.waitid(os.P_PIDFD, leader, os.WEXITED | os.WNOHANG | os.WNOWAIT) is None:
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(0.02)
                self.assertTrue(pid_file.is_file())
                self.assertIn(int(pid_file.read_text()), perf_media_io_trace.session_members(child.pid))
                perf_media_io_trace.terminate_owned(child)
                self.assertEqual(child.returncode, 0)
            finally:
                if child.returncode is None:
                    perf_media_io_trace.terminate_owned(child)
                os.close(leader)


if __name__ == "__main__":
    unittest.main()
