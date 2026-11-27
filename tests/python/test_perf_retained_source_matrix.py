"""Ownership, limits, and data-preservation tests for retained-source workloads."""
import json
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
import perf_retained_source_matrix as matrix

SOURCE = "melodee_template_" + "a" * 32
ADMIN = "postgres://localhost/postgres"


def environment(migration):
    return {"MELODEE_TEST_DATABASE_URL": ADMIN,
        "MELODEE__DATABASE__URL": matrix.test_database.database_url(ADMIN, SOURCE),
        "MELODEE_PERF_OWNED_DATABASE": SOURCE,
        "MELODEE_PERF_FIXTURE_PID": str(os.getppid()),
        "MELODEE_PERF_FIXTURE_SHA256": matrix.file_digest(ROOT / "scripts/perf_representative.py"),
        "MELODEE_PERF_DATABASE_HELPER_SHA256": matrix.file_digest(ROOT / "scripts/test_database.py"),
        "MELODEE_PERF_MIGRATION_CLI_SHA256": matrix.file_digest(migration)}


class RetainedMatrixTests(unittest.TestCase):
    def test_help_is_safe(self):
        p = subprocess.run([sys.executable, str(ROOT / "scripts/perf_retained_source_matrix.py"), "--help"],
                           env={}, capture_output=True, check=False)
        self.assertEqual(p.returncode, 0)

    def test_rates_and_item_counts_are_bounded(self):
        for rates, items, seconds, timeout in (([], 1, 5, 20), ([8, 8], 1, 5, 20),
                                                ([257], 1, 5, 20), ([8], 17, 5, 20),
                                                ([8], 1, 121, 20), ([8], 1, 5, 1201)):
            with self.assertRaises(ValueError):
                matrix.validate_options(rates, items, seconds, timeout, 5, 8)
        with self.assertRaises(ValueError):
            matrix.seed_statement(17)
        self.assertIn("generate_series(1,4)", matrix.seed_statement(4))

    def test_unowned_target_fails_before_database_or_process(self):
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(matrix, "sql") as sql, \
             mock.patch.object(matrix.subprocess, "run") as child:
            with self.assertRaises(ValueError):
                matrix.run([8], 1, 5, 20, 5, 8, Path("server"), Path("proof"), Path("cli"),
                           Path(directory), Path(directory) / "out", 3, 4, 3,
                           {"MELODEE__DATABASE__URL": "postgres://host/production"})
            sql.assert_not_called()
            child.assert_not_called()

    def test_case_rejects_unregistered_or_parent_database_before_work(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cli = root / "cli"
            cli.write_bytes(b"cli")
            env = environment(cli)
            with mock.patch.object(matrix, "query") as query, mock.patch.object(matrix, "sql") as sql:
                with self.assertRaises(ValueError):
                    matrix.run_case(8, 1, 5, 20, 5, 8, root / "server", root, env,
                                    root / "out", set(), SOURCE, ADMIN)
                with self.assertRaises(ValueError):
                    matrix.run_case(8, 1, 5, 20, 5, 8, root / "server", root, env,
                                    root / "out", {SOURCE}, SOURCE, ADMIN)
                query.assert_not_called()
                sql.assert_not_called()

    def test_existing_output_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cli = root / "cli"
            cli.write_bytes(b"cli")
            output = root / "out"
            output.mkdir()
            (output / "keep").write_text("original")
            with mock.patch.object(matrix, "sql") as sql:
                with self.assertRaises(ValueError):
                    matrix.run([8], 1, 5, 20, 5, 8, Path("server"), Path("proof"), cli,
                               root, output, 3, 4, 3, environment(cli))
                sql.assert_not_called()
            self.assertEqual((output / "keep").read_text(), "original")

    def test_source_verification_detects_same_size_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            file = Path(directory) / "owned"
            file.write_bytes(b"abcd")
            entries = [{"path": file, "bytes": 4, "sha256": matrix.file_digest(file)}]
            self.assertTrue(matrix.verify_media(entries))
            file.write_bytes(b"wxyz")
            self.assertFalse(matrix.verify_media(entries))

    def test_retained_measurement_requires_preserved_pending_holds(self):
        held = {"total": 2, "retrying": 0, "active": 2, "superseded": 0, "correct_holds": 2}
        self.assertTrue(matrix.expected_retained_state(held, 2, 8, True))
        for change in ({"correct_holds": 1}, {"superseded": 1}, {"retrying": 1}, {"active": 1}):
            self.assertFalse(matrix.expected_retained_state({**held, **change}, 2, 8, True))
        self.assertFalse(matrix.expected_retained_state(held, 2, 8, False))

    def test_topology_drift_is_rejected_before_sql(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cli = root / "cli"
            cli.write_bytes(b"cli")
            env = {**environment(cli), "MELODEE_TEST_DATABASE_URL": "postgres://other/postgres"}
            with mock.patch.object(matrix, "sql") as sql:
                with self.assertRaises(ValueError):
                    matrix.run([8], 1, 5, 20, 5, 8, Path("server"), Path("proof"), cli,
                               root, root / "out", 3, 4, 3, env)
                sql.assert_not_called()

    def test_interruption_and_lost_create_ack_recover_from_manifest(self):
        record = ownership_record()
        for exception in (InterruptedError("SIGINT"), InterruptedError("SIGTERM"), TimeoutError("lost ack")):
            with self.subTest(exception=exception), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "ownership.json"
                calls = []
                def fake_sql(_url, statement):
                    if statement.startswith("SELECT EXISTS"):
                        return "f"
                    if statement.startswith("CREATE DATABASE"):
                        calls.append("create")
                        raise exception
                    return ""
                def recover(observed, _admin):
                    self.assertEqual(matrix.owned_clone.read_manifest(path), observed)
                    calls.append("cleanup_verified")
                    return "dropped_verified"
                with mock.patch.object(matrix, "sql", side_effect=fake_sql), \
                     mock.patch.object(matrix.owned_clone, "server_identity", return_value=record["server"]), \
                     mock.patch.object(matrix.owned_clone, "recover", side_effect=recover):
                    with self.assertRaises(type(exception)):
                        matrix.run_owned_clone(ADMIN, ADMIN, record, path, lambda _name: self.fail("action"))
                self.assertEqual(calls, ["create", "cleanup_verified"])
                self.assertFalse(path.exists())

    def test_uncertain_absence_and_cleanup_error_keep_manifest(self):
        for outcome in ("absent", RuntimeError("unreachable")):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "ownership.json"
                def fake_sql(_url, statement):
                    if statement.startswith("SELECT EXISTS"):
                        return "f"
                    if statement.startswith("CREATE DATABASE"):
                        raise TimeoutError("lost ack")
                    return ""
                with mock.patch.object(matrix, "sql", side_effect=fake_sql), \
                     mock.patch.object(matrix.owned_clone, "server_identity", return_value=ownership_record()["server"]), \
                     mock.patch.object(matrix.owned_clone, "recover", side_effect=outcome if isinstance(outcome, Exception) else None, return_value=outcome):
                    with self.assertRaises((TimeoutError, RuntimeError)):
                        matrix.run_owned_clone(ADMIN, ADMIN, ownership_record(), path, lambda _name: True)
                self.assertEqual(matrix.owned_clone.read_manifest(path), ownership_record())

    def test_existing_or_inaccessible_candidate_never_attempts_create_or_drop(self):
        for exists in ("t", RuntimeError("connection failed")):
            with self.subTest(exists=exists), tempfile.TemporaryDirectory() as directory:
                statements = []
                def sql(_url, statement):
                    statements.append(statement)
                    if statement.startswith("SELECT EXISTS"):
                        if isinstance(exists, Exception):
                            raise exists
                        return exists
                    return ""
                with mock.patch.object(matrix, "sql", side_effect=sql), \
                     mock.patch.object(matrix.owned_clone, "server_identity", return_value=ownership_record()["server"]), \
                     mock.patch.object(matrix.owned_clone, "recover") as recover:
                    with self.assertRaises(RuntimeError):
                        matrix.run_owned_clone(ADMIN, ADMIN, ownership_record(), Path(directory) / "manifest", lambda _n: True)
                    recover.assert_not_called()
                self.assertFalse(any(s.startswith("CREATE DATABASE") or s.startswith("DROP DATABASE") for s in statements))

    def test_create_commits_then_ack_is_lost_and_verified_cleanup_runs(self):
        record = ownership_record()
        exists = False
        statements = []
        proof = {"database": record["name"], "marker": record["token"], "oid": 42, **record["server"]}
        def sql(_url, statement):
            nonlocal exists
            statements.append(statement)
            if statement.startswith("SELECT EXISTS"):
                return "t" if exists else "f"
            if statement.startswith("CREATE DATABASE"):
                exists = True
                raise TimeoutError("acknowledgement lost")
            if "json_build_object" in statement:
                return json.dumps(proof)
            if "SELECT oid::bigint" in statement:
                return "42"
            if statement.startswith("DROP DATABASE"):
                exists = False
            return ""
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(matrix, "sql", side_effect=sql), \
             mock.patch.object(matrix.owned_clone, "sql", side_effect=sql), \
             mock.patch.object(matrix.owned_clone, "server_identity", return_value=record["server"]):
            path = Path(directory) / "manifest"
            with self.assertRaises(TimeoutError):
                matrix.run_owned_clone(ADMIN, ADMIN, record, path, lambda _n: self.fail("action"))
            self.assertFalse(exists)
            self.assertFalse(path.exists())
            self.assertEqual(sum(s.startswith("DROP DATABASE") for s in statements), 1)

    def test_real_sigint_and_sigterm_invoke_owned_cleanup(self):
        child_code = '''
import json, pathlib, sys, time
sys.path.insert(0, sys.argv[1])
import perf_retained_source_matrix as m
root = pathlib.Path(sys.argv[2]); record = json.loads(sys.argv[3])
m.sql = lambda _url, statement: "f" if statement.startswith("SELECT EXISTS") else ""
m.owned_clone.server_identity = lambda _url: record["server"]
m.owned_clone.verify_candidate = lambda *_args: 42
def recover(*_args):
    (root / "cleanup").write_text("called")
    return "dropped_verified"
m.owned_clone.recover = recover
m.install_signal_handlers()
def action(_name):
    (root / "ready").write_text("ready")
    while True: time.sleep(0.05)
try:
    m.run_owned_clone("postgres://localhost/postgres", "postgres://localhost/postgres", record, root / "manifest", action)
except InterruptedError:
    sys.exit(0)
'''
        for signum in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(signum=signum), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                child = subprocess.Popen([sys.executable, "-c", child_code, str(ROOT / "scripts"),
                                          directory, json.dumps(ownership_record())],
                                         stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                try:
                    deadline = time.monotonic() + 10
                    while not (root / "ready").exists() and time.monotonic() < deadline:
                        if child.poll() is not None:
                            self.fail("signal test child exited before ready")
                        time.sleep(0.02)
                    self.assertTrue((root / "ready").exists())
                    child.send_signal(signum)
                    self.assertEqual(child.wait(timeout=10), 0)
                    self.assertTrue((root / "cleanup").exists())
                    self.assertFalse((root / "manifest").exists())
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=10)
                    if child.stderr:
                        child.stderr.close()


def ownership_record():
    return {"version": 1, "name": "melodee_template_" + "b" * 32, "source": SOURCE,
            "schema": "perf_owner_" + "c" * 32, "token": "d" * 64,
            "endpoint": {"host": "localhost", "port": 5432},
            "server": {"system_id": "12345", "server_address": "127.0.0.1",
                       "server_port": 5432, "in_recovery": False}}


if __name__ == "__main__":
    unittest.main()
