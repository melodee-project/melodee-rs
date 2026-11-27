"""Marker-verified disposable database cleanup never trusts a name alone."""
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
import perf_owned_clone as clone

ADMIN = "postgres://localhost/postgres"
SERVER = {"system_id": "12345", "server_address": "127.0.0.1", "server_port": 5432, "in_recovery": False}
RECORD = {"version": 1, "name": "melodee_template_" + "b" * 32,
          "source": "melodee_template_" + "a" * 32, "schema": "perf_owner_" + "c" * 32,
          "token": "d" * 64, "endpoint": {"host": "localhost", "port": 5432}, "server": SERVER}


class OwnedCloneTests(unittest.TestCase):
    def test_help_does_not_contact_database(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_owned_clone.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_manifest_is_private_and_exclusive(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest"
            clone.write_manifest(path, RECORD)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(clone.read_manifest(path), RECORD)
            with self.assertRaises(ValueError):
                clone.write_manifest(path, RECORD)
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                clone.read_manifest(path)

    def test_mismatch_and_inaccessible_database_never_dropped(self):
        for fault in ("marker", "server", "oid", "connection"):
            with self.subTest(fault=fault):
                statements = []
                proof = {"database": RECORD["name"], "marker": RECORD["token"], "oid": 42, **SERVER}
                if fault == "marker":
                    proof["marker"] = "e" * 64
                if fault == "server":
                    proof["system_id"] = "999"
                def sql(_url, statement):
                    statements.append(statement)
                    if "SELECT EXISTS" in statement:
                        return "t"
                    if "json_build_object" in statement:
                        if fault == "connection":
                            raise RuntimeError("connection failure")
                        return json.dumps(proof)
                    if "SELECT oid::bigint" in statement:
                        return "43" if fault == "oid" else "42"
                    self.fail("unexpected SQL")
                with mock.patch.object(clone, "server_identity", return_value=SERVER), mock.patch.object(clone, "sql", side_effect=sql):
                    with self.assertRaises((ValueError, RuntimeError)):
                        clone.recover(RECORD, ADMIN)
                self.assertFalse(any(s.startswith("DROP DATABASE") for s in statements))

    def test_recovery_drops_only_after_independent_marker_and_oid_checks(self):
        statements = []
        proof = {"database": RECORD["name"], "marker": RECORD["token"], "oid": 42, **SERVER}
        checks = iter(["t", "f"])
        def sql(_url, statement):
            statements.append(statement)
            if "SELECT EXISTS" in statement:
                return next(checks)
            if "json_build_object" in statement:
                return json.dumps(proof)
            if "SELECT oid::bigint" in statement:
                return "42"
            if statement.startswith("DROP DATABASE"):
                return ""
            self.fail("unexpected SQL")
        with mock.patch.object(clone, "server_identity", return_value=SERVER), mock.patch.object(clone, "sql", side_effect=sql):
            self.assertEqual(clone.recover(RECORD, ADMIN), "dropped_verified")
        self.assertTrue(any("ownership" in s for s in statements))
        self.assertEqual(sum(s.startswith("DROP DATABASE") for s in statements), 1)

    def test_server_mismatch_and_endpoint_overrides_fail_closed(self):
        with mock.patch.object(clone, "server_identity", return_value={**SERVER, "system_id": "999"}), \
             mock.patch.object(clone, "sql") as sql:
            with self.assertRaises(ValueError):
                clone.recover(RECORD, ADMIN)
            sql.assert_not_called()
        with self.assertRaises(ValueError):
            clone.endpoint(ADMIN + "?host=other")


if __name__ == "__main__":
    unittest.main()
