"""Ownership and sample semantics for the mixed database workload."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_db_competition as perf


class DatabaseCompetitionTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_db_competition.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_unowned_database_rejected_before_io(self):
        with mock.patch.object(perf, "query") as query, mock.patch.object(perf, "start_server") as server:
            with self.assertRaises(ValueError):
                perf.run("mixed", 1, 20, 8, Path("/missing"), Path("/missing"),
                         {"MELODEE__DATABASE__URL": "postgres://host/production"})
        query.assert_not_called()
        server.assert_not_called()

    def test_resource_limits_fail_closed(self):
        for args in (("invalid", 1, 20, 8), ("mixed", 0, 20, 8), ("mixed", 201, 20, 8),
                     ("mixed", 1, 19, 8), ("mixed", 1, 1201, 8), ("mixed", 1, 20, 1),
                     ("mixed", 1, 20, 65)):
            with self.assertRaises(ValueError):
                perf.validate_options(*args)

    def test_existing_output_is_never_overwritten(self):
        name = "melodee_template_" + "a" * 32
        env = {"MELODEE__DATABASE__URL": "postgres://example/" + name,
               "MELODEE_PERF_OWNED_DATABASE": name}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "evidence.json"
            output.write_text("original")
            with mock.patch.object(perf, "query") as query:
                with self.assertRaises(ValueError):
                    perf.run("mixed", 1, 20, 8, Path("/missing"), Path("/missing"), env, output=output)
            query.assert_not_called()
            self.assertEqual(output.read_text(), "original")

    def test_private_root_prevents_parent_dotenv_and_host_environment_is_filtered(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "owned"
            root.mkdir()
            perf.prepare_root(root)
            self.assertTrue((root / ".env").is_file())
            self.assertEqual((root / ".env").read_text(), "")
        from perf_ingress_workload import restrict_server_environment
        filtered = restrict_server_environment({"PATH": "/bin", "PRIVATE_TOKEN": "secret",
                                                "MELODEE__STORAGE__ROOT": "/private/fixture"})
        self.assertEqual(filtered, {"PATH": "/bin", "MELODEE__STORAGE__ROOT": "/private/fixture"})

    def test_port_selection_uses_loopback_only(self):
        listener = mock.MagicMock()
        listener.__enter__.return_value = listener
        listener.getsockname.return_value = ("127.0.0.1", 4567)
        with mock.patch.object(perf.socket, "socket", return_value=listener):
            self.assertEqual(perf.unused_loopback_port(), 4567)
        listener.bind.assert_called_once_with(("127.0.0.1", 0))

    def test_canary_scan_detects_chunk_boundary_without_exposing_it(self):
        canary = "unique-isolation-canary-value"
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "server.log"
            log.write_bytes(b"x" * (64 * 1024 - 4) + canary.encode() + b"z")
            self.assertTrue(perf.log_contains_canary(log, canary))
            self.assertFalse(perf.log_contains_canary(log, "different-isolation-canary"))
            with self.assertRaises(ValueError):
                perf.log_contains_canary(log, "short")

    def test_overlap_samples_are_wholly_contained_and_errors_counted(self):
        samples = [{"started_seconds": 0, "finished_seconds": 2, "seconds": 2, "status": 200, "error": None},
                   {"started_seconds": 1, "finished_seconds": 2, "seconds": 1, "status": 200, "error": None},
                   {"started_seconds": 2, "finished_seconds": 4, "seconds": 2, "status": None, "error": "TimeoutError"}]
        self.assertEqual(perf.within(samples, 1, 3), [samples[1]])
        self.assertEqual(perf.within(samples, None, None), [])
        self.assertEqual(perf.summary(samples)["errors"], 1)
        self.assertEqual(perf.summary(samples)["latency_seconds"]["p50"], 1)

    def test_request_does_not_emit_secrets_in_error(self):
        connection = mock.Mock()
        connection.request.side_effect = TimeoutError("token-secret")
        with mock.patch.object(perf.http.client, "HTTPConnection", return_value=connection):
            result = perf.request(1234, "GET", "/api/admin/v1/stats", "token-secret")
        self.assertEqual(result[3], "TimeoutError")
        self.assertNotIn("token-secret", str(result))
        connection.close.assert_called_once()

    def test_rebuild_conflict_only_adopts_explicit_in_progress_response(self):
        with mock.patch.object(perf, "request", return_value=(409, b'{"code":"rebuild_in_progress"}', 0.1, None)):
            self.assertFalse(perf.issue_rebuild(1234, "token"))
        with mock.patch.object(perf, "request", return_value=(409, b'{"code":"search_disabled"}', 0.1, None)):
            with self.assertRaises(RuntimeError):
                perf.issue_rebuild(1234, "token")


if __name__ == "__main__":
    unittest.main()
