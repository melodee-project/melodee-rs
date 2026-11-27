"""Safety and no-loss assertions for the real-HTTP ingress workload."""
import contextlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
import signal
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_ingress_workload as perf


class IngressWorkloadTests(unittest.TestCase):
    def test_server_rejects_non_loopback_bind_before_spawning(self):
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(perf.subprocess, "Popen") as popen:
            with self.assertRaisesRegex(ValueError, "loopback"):
                perf.start_server(Path("unused"), Path(directory), "postgres://fixture", 0, False,
                                  {"MELODEE__SERVER__BIND": "0.0.0.0:12345"})
        popen.assert_not_called()

    def test_help_never_contacts_database_or_runs_a_process(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_ingress_workload.py"), "--help"],
                                env={}, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0)
        self.assertIn("--connection-limit", result.stdout)

    def test_unowned_database_rejected_before_any_io(self):
        with mock.patch.object(perf, "query") as query, mock.patch.object(perf.subprocess, "Popen") as popen:
            with self.assertRaises(ValueError):
                perf.run({"MELODEE__DATABASE__URL": "postgres://host/prod"}, Path("/missing"), [4], 2, 2, 1)
        query.assert_not_called()
        popen.assert_not_called()

    def test_limits_and_worker_counts_are_explicitly_bounded(self):
        for args in [([], 1, 1, 1), ([4, 4], 1, 1, 1), ([129], 1, 1, 1),
                     ([4], 65, 1, 1), ([4], 1, 65, 1), ([4], 1, 1, 16),
                     ([4], 1, 1, 1, 29), ([4], 1, 1, 1, 901),
                     ([2], 1, 1, 1, 30, 1), ([4], 1, 1, 1, 30, 17)]:
            with self.assertRaises(ValueError):
                perf.validate_options(*args)
        perf.validate_options([0, 4, 8], 16, 16, 3)

    def test_percentiles_are_nearest_rank_and_preserve_small_samples(self):
        self.assertEqual(perf.percentiles([4, 1, 3, 2]), {"p50": 2, "p95": 4, "p99": 4})
        with self.assertRaises(ValueError):
            perf.percentiles([])

    def test_storage_description_does_not_expose_owned_path(self):
        with tempfile.TemporaryDirectory(prefix="private-test-root-") as directory:
            description = perf.temporary_storage_info(Path(directory))
        self.assertEqual(set(description), {"filesystem", "allocation_unit_bytes"})
        self.assertNotIn(directory, str(description))

    @unittest.skipUnless(os.name == "posix", "process groups are POSIX-specific")
    def test_cleanup_signals_only_the_expected_owned_session(self):
        process = mock.Mock(pid=1234)
        process.poll.return_value = None
        log = mock.Mock()
        with mock.patch.object(perf.os, "getpgid", return_value=1234), \
             mock.patch.object(perf.os, "getsid", return_value=1234), \
             mock.patch.object(perf.os, "killpg") as kill:
            perf.stop_server(process, log)
        kill.assert_called_once_with(1234, signal.SIGTERM)
        process.wait.assert_called_once_with(timeout=15)
        log.close.assert_called_once()

    @unittest.skipUnless(os.name == "posix", "process groups are POSIX-specific")
    def test_cleanup_fails_closed_on_changed_process_topology(self):
        process = mock.Mock(pid=1234)
        process.poll.return_value = None
        log = mock.Mock()
        with mock.patch.object(perf.os, "getpgid", return_value=9999), \
             mock.patch.object(perf.os, "getsid", return_value=1234), \
             mock.patch.object(perf.os, "killpg") as kill:
            with self.assertRaisesRegex(RuntimeError, "topology"):
                perf.stop_server(process, log)
        kill.assert_not_called()
        log.close.assert_called_once()

    def test_server_environment_discards_host_server_settings(self):
        with mock.patch.dict(os.environ, {"MELODEE__STORAGE__ROOT": "/unrelated", "MELODEE_BIND": "0.0.0.0:1", "RUST_LOG": "trace"}, clear=True):
            env = perf.clean_server_environment(Path("/tmp/owned"), "postgres://fixture", 4, False)
        self.assertEqual(env["MELODEE__SERVER__BIND"], "127.0.0.1:0")
        self.assertEqual(env["MELODEE__SERVER__MAX_CONNECTIONS"], "4")
        self.assertEqual(env["MELODEE__STORAGE__ROOT"], "/tmp/owned/storage")
        self.assertEqual(env["MELODEE__INGEST__ENABLED"], "false")
        self.assertNotIn("MELODEE_BIND", env)
        self.assertNotIn("RUST_LOG", env)

    def test_204_without_persisted_play_is_a_failed_workload(self):
        def fake_stream(_port, _token, _track, started, stop):
            started.set()
            stop.wait(2)
            return {"status": 200, "ttfb_seconds": 0.01, "bytes": 1}

        def fake_request(_port, method, _path, _token=None, _body=None):
            return (204 if method == "POST" else 200, b"", 0.01)

        with mock.patch.object(perf, "slow_stream", side_effect=fake_stream), \
             mock.patch.object(perf, "TrackLock", return_value=contextlib.nullcontext()), \
             mock.patch.object(perf, "http_request", side_effect=fake_request), \
             mock.patch.object(perf, "query", side_effect=[{"count": 0, "distinct": 0},
                                                          {"plays": 0, "track_popularity": 0}]):
            result = perf.one_case(123, "fixture", "token", "7bb32617-d1e7-4538-b0db-523baf7262bd",
                                   "public", "ingress-" + "a" * 32 + "-0", 4, 2, 2, 0.01, 0)
        self.assertFalse(result["success"])
        self.assertIn("Jellyfin status/play mismatch", result["issues"])

    def test_database_identifiers_are_rejected_before_subprocesses(self):
        with mock.patch.object(perf.subprocess, "Popen") as popen:
            with self.assertRaises(ValueError):
                perf.TrackLock("fixture", "not-a-uuid') OR true --")
            with self.assertRaises(ValueError):
                perf.one_case(1, "fixture", "token", "track", "public", "invalid%", 4, 2, 2, 1)
        popen.assert_not_called()

    def test_stale_popularity_is_a_failed_workload(self):
        def fake_stream(_port, _token, _track, started, stop):
            started.set()
            stop.wait(2)
            return {"status": 200, "ttfb_seconds": 0.01, "bytes": 1}

        def fake_request(_port, method, _path, _token=None, _body=None):
            return (204 if method == "POST" else 200, b"", 0.01)

        with mock.patch.object(perf, "slow_stream", side_effect=fake_stream), \
             mock.patch.object(perf, "TrackLock", return_value=contextlib.nullcontext()), \
             mock.patch.object(perf, "http_request", side_effect=fake_request), \
             mock.patch.object(perf, "query", side_effect=[{"count": 2, "distinct": 2},
                                                          {"plays": 2, "track_popularity": 1}]):
            result = perf.one_case(123, "fixture", "token", "7bb32617-d1e7-4538-b0db-523baf7262bd", "public",
                                   "ingress-" + "a" * 32 + "-0", 4, 2, 2, 0.01, 0)
        self.assertFalse(result["success"])
        self.assertIn("track popularity does not equal committed plays", result["issues"])

    def test_failed_request_is_recorded_without_secret_or_false_success(self):
        with mock.patch.object(perf, "http_request", side_effect=TimeoutError("fixture secret")):
            sample = perf.observed_request(1, "POST", "/test", "fixture secret")
        self.assertIsNone(sample["status"])
        self.assertEqual(sample["error"], "TimeoutError")
        self.assertNotIn("fixture secret", str(sample))

    def test_idle_connection_is_closed_when_keepalive_is_not_supported(self):
        connection = mock.Mock()
        response = connection.getresponse.return_value
        response.status = 200
        response.will_close = True
        with mock.patch.object(perf.http.client, "HTTPConnection", return_value=connection):
            with self.assertRaisesRegex(RuntimeError, "keep-alive"):
                perf.one_case(123, "fixture", "token", "track", "public",
                              "ingress-" + "a" * 32 + "-0", 4, 2, 2, 0.01, 1)
        connection.close.assert_called_once()

    def test_stream_ttfb_is_recorded_after_receiving_a_media_byte(self):
        connection = mock.Mock()
        response = connection.getresponse.return_value
        response.status = 200
        response.read.return_value = b"a"
        started = threading.Event()
        stop = threading.Event()
        stop.set()
        with mock.patch.object(perf.http.client, "HTTPConnection", return_value=connection):
            result = perf.slow_stream(123, "token", "track", started, stop)
        response.read.assert_called_once_with(1)
        self.assertGreaterEqual(result["ttfb_seconds"], result["time_to_headers_seconds"])
        self.assertEqual(result["bytes"], 1)
        self.assertTrue(started.is_set())
        connection.close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
