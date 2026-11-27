"""Safety and lifecycle tests for the disposable comparison runner."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import run_perf_scrobble_comparison as comparison

SOURCE = "melodee_template_" + "a" * 32
ADMIN = "postgres://test:test@localhost:5433/postgres"
ENV = {"MELODEE_TEST_DATABASE_URL": ADMIN,
       "MELODEE__DATABASE__URL": ADMIN.replace("/postgres", "/" + SOURCE),
       "MELODEE_PERF_OWNED_DATABASE": SOURCE}


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.control = Path(self.directory.name) / "control"
        self.treatment = Path(self.directory.name) / "treatment"
        for path in (self.control, self.treatment):
            path.write_text("binary", encoding="utf8")
            path.chmod(0o700)

    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/run_perf_scrobble_comparison.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_rejects_invalid_inputs_before_database_access(self):
        for control, treatment, rounds, env in [
            (self.control, self.control, 1, ENV),
            (self.control, Path(self.directory.name) / "missing", 1, ENV),
            (self.control, self.treatment, 0, ENV),
            (self.control, self.treatment, 11, ENV),
            (self.control, self.treatment, 1, {**ENV, "MELODEE_PERF_OWNED_DATABASE": "postgres"}),
            (self.control, self.treatment, 1, {**ENV, "MELODEE__DATABASE__URL": ENV["MELODEE__DATABASE__URL"] + "?dbname=postgres"}),
            (self.control, self.treatment, 1, {**ENV, "MELODEE_TEST_DATABASE_URL": ADMIN.replace("localhost", "other")}),
        ]:
            with self.subTest(env=env, rounds=rounds), mock.patch.object(comparison.test_database, "sql") as sql:
                with self.assertRaises((ValueError, OSError)):
                    comparison.run(control, treatment, rounds, env)
                sql.assert_not_called()

    def test_sequential_clones_and_cleanup_on_failure(self):
        created = []
        dropped = []
        arms = []
        def sql(url, statement):
            if statement == "SELECT current_database()":
                return url.rsplit("/", 1)[1]
            self.assertIn(f'TEMPLATE "{SOURCE}"', statement)
            name = statement.split('"')[1]
            self.assertNotEqual(name, SOURCE)
            created.append(name)
            return ""
        def drop(admin, name, registry):
            self.assertEqual(admin, ADMIN)
            self.assertIn(name, registry)
            self.assertNotEqual(name, SOURCE)
            dropped.append(name)
            registry.remove(name)
        def run(command, **kwargs):
            env = kwargs["env"]
            self.assertEqual(env["MELODEE_PERF_OWNED_DATABASE"], created[-1])
            self.assertTrue(env["MELODEE__DATABASE__URL"].endswith("/" + created[-1]))
            self.assertEqual(len(created) - len(dropped), 1)
            arms.append(Path(command[0]).name)
            return subprocess.CompletedProcess(command, 1 if len(arms) == 3 else 0)
        with mock.patch.object(comparison.test_database, "sql", side_effect=sql), \
             mock.patch.object(comparison.test_database, "drop_owned", side_effect=drop), \
             mock.patch.object(comparison.subprocess, "run", side_effect=run):
            self.assertEqual(comparison.run(self.control, self.treatment, 2, ENV), 1)
        self.assertEqual(arms, ["control", "treatment", "treatment"])
        self.assertEqual(created, dropped)

    def test_three_way_order(self):
        hybrid = Path(self.directory.name) / "hybrid"
        hybrid.write_text("binary", encoding="utf8")
        hybrid.chmod(0o700)
        arms = []
        def sql(url, statement):
            return url.rsplit("/", 1)[1] if statement == "SELECT current_database()" else ""
        def drop(admin, name, registry):
            self.assertNotEqual(name, SOURCE)
            registry.remove(name)
        def run(command, **kwargs):
            arms.append(Path(command[0]).name)
            return subprocess.CompletedProcess(command, 0)
        with mock.patch.object(comparison.test_database, "sql", side_effect=sql), \
             mock.patch.object(comparison.test_database, "drop_owned", side_effect=drop), \
             mock.patch.object(comparison.subprocess, "run", side_effect=run):
            self.assertEqual(comparison.run(self.control, self.treatment, 1, ENV, hybrid), 0)
        self.assertEqual(arms, ["control", "treatment", "hybrid", "hybrid", "treatment", "control"])

    def test_identity_mismatch_cleans_up(self):
        calls = 0
        def sql(url, statement):
            nonlocal calls
            if statement == "SELECT current_database()":
                calls += 1
                return SOURCE if calls == 1 else "wrong"
            return ""
        with mock.patch.object(comparison.test_database, "sql", side_effect=sql), \
             mock.patch.object(comparison.test_database, "drop_owned") as drop:
            with self.assertRaises(RuntimeError):
                comparison.run(self.control, self.treatment, 1, ENV)
            self.assertEqual(drop.call_count, 1)
            self.assertNotEqual(drop.call_args.args[1], SOURCE)


if __name__ == "__main__":
    unittest.main()
