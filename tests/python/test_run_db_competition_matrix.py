"""Safety checks for the sequential, invocation-owned DB workload matrix."""
import json
import hashlib
import contextlib
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import run_db_competition_matrix as matrix

SOURCE = "melodee_template_" + "a" * 32
ADMIN = "postgres://test:test@localhost:5433/postgres"


def environment(migration):
    return {"MELODEE_TEST_DATABASE_URL": ADMIN,
            "MELODEE__DATABASE__URL": ADMIN.replace("/postgres", "/" + SOURCE),
            "MELODEE_PERF_OWNED_DATABASE": SOURCE,
            "MELODEE_PERF_FIXTURE_SHA256": matrix.file_digest(ROOT / "scripts/perf_representative.py"),
            "MELODEE_PERF_DATABASE_HELPER_SHA256": matrix.file_digest(ROOT / "scripts/test_database.py"),
            "MELODEE_PERF_MIGRATION_CLI_SHA256": hashlib.sha256(migration.read_bytes()).hexdigest()}


class MatrixTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/run_db_competition_matrix.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_rejects_duplicate_or_unbounded_cases(self):
        for modes, pools, artists in (([], [8], 10), (["idle", "idle"], [8], 10),
                                      (["idle"], [8, 8], 10), (["idle"], [1], 10),
                                      (["idle"], [8], 1_000_001)):
            with self.assertRaises(ValueError):
                matrix.validate_matrix(modes, pools, artists, 10, 2, 1, 20)

    def test_existing_directory_is_untouched_before_any_work(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "results"
            output.mkdir()
            (output / "keep").write_text("original")
            migration = Path(temp) / "migration"
            migration.write_bytes(b"migration")
            with mock.patch.object(matrix.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    matrix.run(["idle"], [8], 10, 10, 2, 1, 20, Path("missing"),
                               Path("missing"), output, migration, Path(temp), False,
                               env=environment(migration))
            run.assert_not_called()
            self.assertEqual((output / "keep").read_text(), "original")

    def test_cases_run_sequentially_and_stop_on_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "server"
            binary.write_bytes(b"binary")
            migration = root / "migration"
            migration.write_bytes(b"migration")
            provenance = root / "build.json"
            provenance.write_text(json.dumps({"binary_sha256": "fixture", "commit": "fixture"}))
            output = root / "results"
            calls = []
            created = []
            dropped = []
            def sql(url, statement):
                if statement == "SELECT current_database()":
                    return url.rsplit("/", 1)[1]
                if "json_build_object" in statement:
                    return json.dumps({"artists": 10, "releases": 10, "tracks": 20, "work": 0, "jobs": 0})
                self.assertIn(f'TEMPLATE "{SOURCE}"', statement)
                created.append(statement.split('"')[1])
                return ""
            def drop(admin, name, registry):
                self.assertEqual(admin, ADMIN)
                self.assertIn(name, registry)
                self.assertNotEqual(name, SOURCE)
                dropped.append(name)
                registry.remove(name)
            def run(mode, _albums, _seconds, _pool, _server, _parent, env, _allow, result_path):
                calls.append(mode)
                self.assertEqual(len(created) - len(dropped), 1)
                self.assertEqual(env["MELODEE_PERF_OWNED_DATABASE"], created[-1])
                self.assertTrue(env["MELODEE__DATABASE__URL"].endswith("/" + created[-1]))
                result_path.write_text("{}")
                return 1 if len(calls) == 2 else 0
            with mock.patch.object(matrix, "verify_provenance"), \
                 mock.patch.object(matrix.test_database, "sql", side_effect=sql), \
                 mock.patch.object(matrix.test_database, "drop_owned", side_effect=drop), \
                 mock.patch.object(matrix, "run_workload", side_effect=run):
                result = matrix.run(["idle", "mixed"], [8, 32], 10, 10, 2, 1, 20,
                                    binary, provenance, output, migration, root, False,
                                    env=environment(migration))
            self.assertEqual(result, 1)
            self.assertEqual(len(calls), 2)
            self.assertEqual(created, dropped)
            self.assertEqual(calls, ["idle", "mixed"])
            report = json.loads((output / "matrix.json").read_text())
            self.assertFalse(report["cases_succeeded"])
            self.assertFalse(report["metrics_complete"])
            self.assertEqual([case["mode"] for case in report["cases"]], ["idle", "mixed"])

    def test_topology_drift_rejected_without_database_access(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            migration = root / "migration"
            migration.write_bytes(b"migration")
            env = {**environment(migration), "MELODEE_TEST_DATABASE_URL": ADMIN.replace("localhost", "other")}
            with mock.patch.object(matrix.test_database, "sql") as sql:
                with self.assertRaises(ValueError):
                    matrix.run(["idle"], [8], 10, 10, 2, 1, 20, Path("missing"),
                               Path("missing"), root / "results", migration, root, False, env=env)
            sql.assert_not_called()

    def test_changed_fixture_source_is_rejected_before_clone(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            server, migration, provenance = root / "server", root / "migration", root / "build.json"
            server.write_bytes(b"server")
            migration.write_bytes(b"migration")
            provenance.write_text(json.dumps({"binary_sha256": "fixture", "commit": "fixture"}))
            env = {**environment(migration), "MELODEE_PERF_FIXTURE_SHA256": "0" * 64}
            with mock.patch.object(matrix, "verify_provenance"), \
                 mock.patch.object(matrix.test_database, "sql") as sql:
                with self.assertRaises(ValueError):
                    matrix.run(["idle"], [8], 10, 10, 2, 1, 20, server, provenance,
                               root / "results", migration, root, False, env=env)
            sql.assert_not_called()

    def test_clone_identity_failure_cleans_only_created_clone(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            server, migration, provenance = root / "server", root / "migration", root / "build.json"
            server.write_bytes(b"server")
            migration.write_bytes(b"migration")
            provenance.write_text(json.dumps({"binary_sha256": "fixture", "commit": "fixture"}))
            current_calls = 0
            def sql(url, statement):
                nonlocal current_calls
                if statement == "SELECT current_database()":
                    current_calls += 1
                    return SOURCE if current_calls == 1 else "wrong"
                if "json_build_object" in statement:
                    return json.dumps({"artists": 10, "releases": 10, "tracks": 20, "work": 0, "jobs": 0})
                return ""
            removed = []
            def drop(_admin, name, registry):
                self.assertNotEqual(name, SOURCE)
                removed.append(name)
                registry.remove(name)
            with mock.patch.object(matrix, "verify_provenance"), \
                 mock.patch.object(matrix.test_database, "sql", side_effect=sql), \
                 mock.patch.object(matrix.test_database, "drop_owned", side_effect=drop):
                with self.assertRaises(RuntimeError):
                    matrix.run(["idle"], [8], 10, 10, 2, 1, 20, server, provenance,
                               root / "results", migration, root, False, env=environment(migration))
            self.assertEqual(len(removed), 1)

    def test_interrupted_workload_cleans_registered_clone(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            server, migration, provenance = root / "server", root / "migration", root / "build.json"
            server.write_bytes(b"server")
            migration.write_bytes(b"migration")
            provenance.write_text(json.dumps({"binary_sha256": "fixture", "commit": "fixture"}))
            created, removed = [], []
            def sql(url, statement):
                if statement == "SELECT current_database()":
                    return url.rsplit("/", 1)[1]
                if "json_build_object" in statement:
                    return json.dumps({"artists": 10, "releases": 10, "tracks": 20, "work": 0, "jobs": 0})
                created.append(statement.split('"')[1])
                return ""
            def drop(_admin, name, registry):
                self.assertNotEqual(name, SOURCE)
                removed.append(name)
                registry.remove(name)
            with mock.patch.object(matrix, "verify_provenance"), \
                 mock.patch.object(matrix.test_database, "sql", side_effect=sql), \
                 mock.patch.object(matrix.test_database, "drop_owned", side_effect=drop), \
                 mock.patch.object(matrix, "run_workload", side_effect=InterruptedError("test")):
                with self.assertRaises(InterruptedError):
                    matrix.run(["idle"], [8], 10, 10, 2, 1, 20, server, provenance,
                               root / "results", migration, root, False, env=environment(migration))
            self.assertEqual(created, removed)

    def test_unacknowledged_clone_create_is_not_deleted(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            server, migration, provenance = root / "server", root / "migration", root / "build.json"
            server.write_bytes(b"server")
            migration.write_bytes(b"migration")
            provenance.write_text(json.dumps({"binary_sha256": "fixture", "commit": "fixture"}))
            def sql(url, statement):
                if statement == "SELECT current_database()":
                    return SOURCE
                if "json_build_object" in statement:
                    return json.dumps({"artists": 10, "releases": 10, "tracks": 20, "work": 0, "jobs": 0})
                raise RuntimeError("lost CREATE acknowledgement")
            error = io.StringIO()
            with mock.patch.object(matrix, "verify_provenance"), \
                 mock.patch.object(matrix.test_database, "sql", side_effect=sql), \
                 mock.patch.object(matrix.test_database, "drop_owned") as drop, \
                 contextlib.redirect_stderr(error):
                with self.assertRaises(RuntimeError):
                    matrix.run(["idle"], [8], 10, 10, 2, 1, 20, server, provenance,
                               root / "results", migration, root, False, env=environment(migration))
            drop.assert_not_called()
            self.assertIn("uncertain_clone_create", error.getvalue())
            self.assertNotIn(SOURCE, error.getvalue())

    def test_successful_baseline_with_missing_metrics_is_not_metric_complete(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            server, migration, provenance = root / "server", root / "migration", root / "build.json"
            server.write_bytes(b"server")
            migration.write_bytes(b"migration")
            provenance.write_text(json.dumps({"binary_sha256": "fixture", "commit": "fixture"}))
            def sql(url, statement):
                if statement == "SELECT current_database()":
                    return url.rsplit("/", 1)[1]
                if "json_build_object" in statement:
                    return json.dumps({"artists": 10, "releases": 10, "tracks": 20, "work": 0, "jobs": 0})
                return ""
            def drop(_admin, name, registry):
                registry.remove(name)
            def workload(_mode, _albums, _seconds, _pool, _server, _parent, _env, allow, output):
                self.assertTrue(allow)
                output.write_text(json.dumps({"required_metrics_missing": ["heartbeat_hold"]}))
                return 0
            with mock.patch.object(matrix, "verify_provenance"), \
                 mock.patch.object(matrix.test_database, "sql", side_effect=sql), \
                 mock.patch.object(matrix.test_database, "drop_owned", side_effect=drop), \
                 mock.patch.object(matrix, "run_workload", side_effect=workload), \
                 contextlib.redirect_stdout(io.StringIO()):
                result = matrix.run(["idle"], [8], 10, 10, 2, 1, 20, server, provenance,
                                    root / "results", migration, root, True, env=environment(migration))
            self.assertEqual(result, 0)
            report = json.loads((root / "results/matrix.json").read_text())
            self.assertTrue(report["cases_succeeded"])
            self.assertFalse(report["metrics_complete"])


if __name__ == "__main__":
    unittest.main()
