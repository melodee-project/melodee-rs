"""Ownership and query checks for the Native Artist count probe."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_native_artist_scope_plan as perf


def valid_env():
    name = "melodee_template_" + "a" * 32
    admin = "postgres://localhost/postgres"
    return {"MELODEE_PERF_OWNED_DATABASE": name,
            "MELODEE__DATABASE__URL": perf.test_database.database_url(admin, name),
            "MELODEE_TEST_DATABASE_URL": admin,
            "MELODEE_PERF_FIXTURE_PID": str(os.getppid()),
            "MELODEE_PERF_FIXTURE_SHA256": perf.file_digest(ROOT / "scripts/perf_representative.py"),
            "MELODEE_PERF_DATABASE_HELPER_SHA256": perf.file_digest(ROOT / "scripts/test_database.py")}


class NativeArtistPlanTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_native_artist_scope_plan.py"), "--help"],
                                capture_output=True, env={}, check=False)
        self.assertEqual(result.returncode, 0)

    def test_unowned_targets_and_bad_limits_fail_before_sql(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(perf.test_database, "sql") as sql:
            for env in ({"MELODEE__DATABASE__URL": "postgres://host/production"},
                        {**valid_env(), "MELODEE_PERF_FIXTURE_PID": "0"},
                        {**valid_env(), "MELODEE_TEST_DATABASE_URL": "postgres://other/postgres"}):
                with self.assertRaises(ValueError):
                    perf.run(env, Path(directory) / "out", 1, 1, 1, 1)
            with self.assertRaises(ValueError):
                perf.run(valid_env(), Path(directory) / "out", 1, 1, 1, 21)
            sql.assert_not_called()

    def test_existing_output_is_never_modified(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(perf.test_database, "sql") as sql:
            output = Path(directory) / "out"
            output.write_text("preserved")
            with self.assertRaises(ValueError):
                perf.run(valid_env(), output, 1, 1, 1, 1)
            self.assertEqual(output.read_text(), "preserved")
            sql.assert_not_called()

    def test_failed_clone_create_never_deletes_unregistered_database(self):
        env = valid_env()
        def sql(url, statement):
            if statement == "SELECT current_database()":
                return env["MELODEE_PERF_OWNED_DATABASE"]
            if statement.startswith("SELECT json_build_object"):
                return json.dumps({"artists": 1, "releases": 1, "tracks": 1, "work": 0, "jobs": 0})
            raise RuntimeError("ambiguous create")
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(perf.test_database, "sql", side_effect=sql), \
             mock.patch.object(perf.test_database, "drop_owned") as drop, \
             mock.patch.object(perf.sys, "stderr"):
            with self.assertRaises(RuntimeError):
                perf.run(env, Path(directory) / "out", 1, 1, 1, 1)
        drop.assert_not_called()

    def test_success_queries_only_registered_clone_and_removes_it(self):
        env = valid_env()
        source_url = env["MELODEE__DATABASE__URL"]
        calls = []
        def sql(url, statement):
            calls.append((url, statement))
            if statement == "SELECT current_database()":
                return url.rsplit("/", 1)[-1]
            if statement.startswith("SELECT json_build_object"):
                return json.dumps({"artists": 1, "releases": 1, "tracks": 1, "work": 0, "jobs": 0})
            if statement.startswith("EXPLAIN"):
                return '[{"Plan": {}, "Execution Time": 1}]'
            if statement.startswith("SELECT coalesce(json_agg"):
                return '[{"id":"fixture","count":1}]'
            return ""
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(perf.test_database, "sql", side_effect=sql), \
             mock.patch.object(perf.test_database, "drop_owned") as drop:
            output = Path(directory) / "out"
            perf.run(env, output, 1, 1, 1, 1)
            self.assertEqual(json.loads(output.read_text())["results"]["after"][0]["row_count"], 1)
        clone = drop.call_args.args[1]
        self.assertNotEqual(clone, env["MELODEE_PERF_OWNED_DATABASE"])
        self.assertEqual(drop.call_args.args[2], {clone})
        measurements = [url for url, statement in calls if statement.startswith(("EXPLAIN", "SELECT coalesce(json_agg"))]
        self.assertTrue(measurements)
        self.assertTrue(all(url.rsplit("/", 1)[-1] == clone for url in measurements))
        self.assertFalse(any(url == source_url and statement.startswith(("INSERT", "UPDATE", "DELETE", "DROP", "CREATE"))
                             for url, statement in calls))

    def test_query_retains_acl_and_filters_both_grouped_inputs(self):
        before, after = perf.query(False), perf.query(True)
        self.assertEqual(before.count("native_user_can_access_release"), 2)
        self.assertEqual(after.count("native_user_can_access_release"), 2)
        self.assertIn("release_artist.role='primary' AND release_artist.artist_id = ANY", after)
        self.assertIn("WHERE association.artist_id = ANY", after)
        self.assertEqual(before.count("count(DISTINCT"), after.count("count(DISTINCT"))


if __name__ == "__main__":
    unittest.main()
