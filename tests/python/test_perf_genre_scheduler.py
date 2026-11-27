"""Scope and result checks for the disposable genre/scheduler workload."""
import json
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_genre_scheduler as perf


class GenreSchedulerTests(unittest.TestCase):
    def test_help_never_connects_or_starts_processes(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_genre_scheduler.py"), "--help"],
                                env={}, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_refuses_unowned_database_before_external_actions(self):
        with mock.patch.object(perf, "query") as query, mock.patch.object(perf, "start_server") as start:
            with self.assertRaises(ValueError):
                perf.run(2, 10, Path(sys.executable), {"MELODEE__DATABASE__URL": "postgres://host/prod"}, None)
        query.assert_not_called()
        start.assert_not_called()

    def test_refuses_existing_items_before_server_start(self):
        name = "melodee_template_" + "d" * 32
        env = {"MELODEE_PERF_OWNED_DATABASE": name, "MELODEE__DATABASE__URL": "postgres://localhost/" + name}
        with mock.patch.object(perf, "query", return_value={"database": name, "items": [1]}), \
             mock.patch.object(perf, "start_server") as start:
            with self.assertRaises(ValueError):
                perf.run(2, 10, Path(sys.executable), env, None)
        start.assert_not_called()

    def test_rejects_unbounded_workload_before_database_access(self):
        name = "melodee_template_" + "e" * 32
        env = {"MELODEE_PERF_OWNED_DATABASE": name, "MELODEE__DATABASE__URL": "postgres://localhost/" + name}
        with mock.patch.object(perf, "query") as query:
            for releases, timeout in ((1, 5), (201, 5), (2, 0), (2, 901)):
                with self.assertRaises(ValueError):
                    perf.run(releases, timeout, Path(sys.executable), env, None)
        query.assert_not_called()

    def test_requires_exact_counts_for_both_surfaces(self):
        expected = [{"surface": surface, "song_count": 2, "release_count": 2}
                    for surface in ("native", "opensubsonic")]
        self.assertTrue(perf.exact_genre({"rows": expected, "actual": expected}, 2))
        self.assertFalse(perf.exact_genre({"rows": expected[:1], "actual": expected}, 2))
        self.assertFalse(perf.exact_genre({"rows": expected, "actual": expected}, 3))

    def test_sampling_timeout_is_censored_and_server_is_cleaned(self):
        name = "melodee_template_" + "f" * 32
        env = {"MELODEE_PERF_OWNED_DATABASE": name, "MELODEE__DATABASE__URL": "postgres://localhost/" + name}
        initial = {"database": name, "items": [], "states": {}}
        empty = {"rows": [], "actual": []}
        with tempfile.TemporaryDirectory() as directory:
            log_path = Path(directory) / "owned.log"
            log_path.write_text("")
            log = mock.Mock(name=str(log_path))
            log.name = str(log_path)
            process = mock.Mock()
            process.poll.return_value = None
            output = io.StringIO()
            timeout = subprocess.TimeoutExpired("psql", 15)
            with mock.patch.object(perf, "query", side_effect=[initial, empty, timeout, initial, empty, {}]), \
                 mock.patch.object(perf, "generate", return_value=10), \
                 mock.patch.object(perf, "start_server", return_value=(process, log)), \
                 mock.patch.object(perf, "stop_server") as stop, \
                 mock.patch("sys.stdout", output):
                self.assertEqual(perf.run(2, 10, Path(sys.executable), env, Path(directory)), 1)
            stop.assert_called_once_with(process, log)
            self.assertTrue(json.loads(output.getvalue())["timed_out"])

    def test_ready_wait_and_log_summary(self):
        items = [{"events": [{"to": "ready", "time": "2026-01-01T00:00:00Z"},
                             {"type": "claimed", "time": "2026-01-01T00:00:02Z"}]}]
        self.assertEqual(perf.event_waits(items), [2.0])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "server.log"
            path.write_text("\n".join(json.dumps({"fields": {"message": "inspection batch completed", "claimed": count}})
                                      for count in (2, 0)))
            self.assertEqual(perf.summarize_log(path)["inspection"],
                             {"passes": 2, "idle_passes": 1, "claimed": 2})


if __name__ == "__main__":
    unittest.main()
