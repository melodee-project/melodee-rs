"""Ownership and no-loss checks for the Jellyfin plan benchmark."""
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
import perf_jellyfin_audio_plan as perf


def valid_env():
    name = "melodee_template_" + "a" * 32
    admin = "postgres://localhost/postgres"
    return {"MELODEE_PERF_OWNED_DATABASE": name,
            "MELODEE__DATABASE__URL": perf.test_database.database_url(admin, name),
            "MELODEE_TEST_DATABASE_URL": admin,
            "MELODEE_PERF_FIXTURE_PID": str(os.getppid()),
            "MELODEE_PERF_FIXTURE_SHA256": perf.file_digest(ROOT / "scripts/perf_representative.py"),
            "MELODEE_PERF_DATABASE_HELPER_SHA256": perf.file_digest(ROOT / "scripts/test_database.py")}


class JellyfinPlanTests(unittest.TestCase):
    def test_help_has_no_side_effects(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_jellyfin_audio_plan.py"), "--help"],
                                capture_output=True, env={}, check=False)
        self.assertEqual(result.returncode, 0)

    def test_unowned_or_mismatched_targets_rejected_before_io(self):
        scenarios = [
            {"MELODEE__DATABASE__URL": "postgres://host/production"},
            {**valid_env(), "MELODEE_PERF_FIXTURE_PID": "0"},
            {**valid_env(), "MELODEE_TEST_DATABASE_URL": "postgres://other/postgres"},
            {**valid_env(), "MELODEE_PERF_FIXTURE_SHA256": "incorrect"},
        ]
        with tempfile.TemporaryDirectory() as directory:
            for env in scenarios:
                with self.subTest(env=env), mock.patch.object(perf.test_database, "sql") as query:
                    with self.assertRaises(ValueError):
                        perf.run(env, Path(directory) / "result.json", 1, 1, 1, 1)
                    query.assert_not_called()

    def test_existing_output_and_bad_limits_rejected_before_db_io(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "result.json"
            output.write_text("preserve")
            with mock.patch.object(perf.test_database, "sql") as query:
                with self.assertRaises(ValueError):
                    perf.run(valid_env(), output, 1, 1, 1, 1)
                with self.assertRaises(ValueError):
                    perf.run(valid_env(), output, 1, 1, 1, 21)
                query.assert_not_called()
            self.assertEqual(output.read_text(), "preserve")

    def test_different_index_is_never_dropped_and_registered_clone_is_removed(self):
        env = valid_env()
        name = env["MELODEE_PERF_OWNED_DATABASE"]
        calls = []
        def sql(url, statement):
            calls.append((url, statement))
            if statement == "SELECT current_database()":
                return url.rsplit("/", 1)[-1]
            if statement.startswith("SELECT json_build_object"):
                return json.dumps({"artists": 1, "releases": 1, "tracks": 1, "work": 0, "jobs": 0})
            if statement.startswith("SELECT coalesce(pg_get_indexdef"):
                return "different definition"
            return ""
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(perf.test_database, "sql", side_effect=sql), \
             mock.patch.object(perf.test_database, "drop_owned") as drop:
            with self.assertRaisesRegex(RuntimeError, "definition"):
                perf.run(env, Path(directory) / "result.json", 1, 1, 1, 1)
        creates = [(url, sql) for url, sql in calls if sql.startswith("CREATE DATABASE")]
        self.assertEqual(len(creates), 1)
        self.assertIn(f'TEMPLATE "{name}"', creates[0][1])
        self.assertFalse(any(sql.startswith("DROP INDEX") for _, sql in calls))
        drop.assert_called_once()
        self.assertNotEqual(drop.call_args.args[1], name)
        self.assertEqual(drop.call_args.args[2], {drop.call_args.args[1]})

    def test_failed_create_never_deletes_unregistered_database(self):
        env = valid_env()
        def sql(url, statement):
            if statement == "SELECT current_database()":
                return env["MELODEE_PERF_OWNED_DATABASE"]
            if statement.startswith("SELECT json_build_object"):
                return json.dumps({"artists": 1, "releases": 1, "tracks": 1, "work": 0, "jobs": 0})
            raise RuntimeError("ambiguous acknowledgment")
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(perf.test_database, "sql", side_effect=sql), \
             mock.patch.object(perf.test_database, "drop_owned") as drop, \
             mock.patch.object(perf.sys, "stderr"):
            with self.assertRaises(RuntimeError):
                perf.run(env, Path(directory) / "result.json", 1, 1, 1, 1)
        drop.assert_not_called()

    def test_comparison_mutates_only_newly_created_clone(self):
        env = valid_env()
        source_url = env["MELODEE__DATABASE__URL"]
        calls = []
        def sql(url, statement):
            calls.append((url, statement))
            if statement == "SELECT current_database()":
                return url.rsplit("/", 1)[-1]
            if statement.startswith("SELECT json_build_object"):
                return json.dumps({"artists": 1, "releases": 1, "tracks": 1, "work": 0, "jobs": 0})
            if statement.startswith("SELECT coalesce(pg_get_indexdef"):
                return perf.INDEX_DEFINITION
            if statement.startswith("SELECT pg_relation_size"):
                return "8192"
            return ""
        sample = [{"page_sha256": "same", "exact_count": 1, "plans": {}}]
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(perf.test_database, "sql", side_effect=sql), \
             mock.patch.object(perf.test_database, "drop_owned") as drop, \
             mock.patch.object(perf, "measure", return_value=sample):
            output = Path(directory) / "result.json"
            perf.run(env, output, 1, 1, 1, 1)
            self.assertEqual(json.loads(output.read_text())["index_bytes"], 8192)
        mutation_urls = [url for url, statement in calls if statement.startswith(("DROP INDEX", "CREATE INDEX"))]
        self.assertEqual(len(mutation_urls), 2)
        self.assertEqual(mutation_urls[0], mutation_urls[1])
        self.assertNotEqual(mutation_urls[0], source_url)
        self.assertEqual(mutation_urls[0].rsplit("/", 1)[-1], drop.call_args.args[1])


if __name__ == "__main__":
    unittest.main()
