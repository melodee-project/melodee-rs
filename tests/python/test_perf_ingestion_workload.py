"""Safety checks for the disposable real-ingestion benchmark."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_ingestion_workload as perf


class IngestionWorkloadTests(unittest.TestCase):
    def test_help_has_no_external_effects(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_ingestion_workload.py"), "--help"],
                                env={}, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_unowned_database_rejected_before_process_or_filesystem_work(self):
        with mock.patch.object(perf, "query") as query, mock.patch.object(perf.subprocess, "Popen") as popen:
            with self.assertRaises(ValueError):
                perf.run(1, 2, 1, Path("/nonexistent"), {"MELODEE__DATABASE__URL": "postgres://host/prod"})
        query.assert_not_called()
        popen.assert_not_called()

    def test_nonempty_owned_fixture_rejected(self):
        name = "melodee_template_" + "a" * 32
        env = {"MELODEE_PERF_OWNED_DATABASE": name, "MELODEE__DATABASE__URL": "postgres://localhost/" + name}
        with mock.patch.object(perf, "query", return_value={"database": name, "items": [1]}), \
             mock.patch.object(perf.subprocess, "Popen") as popen:
            with self.assertRaises(ValueError):
                perf.run(1, 2, 1, Path(sys.executable), env)
        popen.assert_not_called()

    def test_server_inherits_no_user_melodee_configuration(self):
        with mock.patch.dict(os.environ, {"MELODEE__INGEST__SOURCE_ROOT": "/unrelated", "MELODEE__PODCAST__WORKER__ENABLED": "true"}, clear=True):
            env = perf.server_environment(Path("/tmp/owned"), "postgres://localhost/example", 2)
        self.assertEqual(env["MELODEE__INGEST__SOURCE_ROOT"], "/tmp/owned/inbound")
        self.assertEqual(env["MELODEE__PODCAST__WORKER__ENABLED"], "false")
        self.assertEqual(env["MELODEE__DISCOVERY__WORKER__ENABLED"], "false")

    def test_reimport_redrops_identical_bytes_from_owned_corpus(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("originals/release", "inbound", "reimport-scratch"):
                (root / name).mkdir(parents=True)
            (root / "originals/release/track.mp3").write_bytes(b"identical media")
            (root / "originals/release/.melodee-ready").write_bytes(b"")
            perf.queue_reimport(root)
            self.assertEqual((root / "inbound/release/track.mp3").read_bytes(), b"identical media")
            self.assertTrue((root / "inbound/release/.melodee-ready").is_file())
            self.assertEqual(list((root / "reimport-scratch").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
