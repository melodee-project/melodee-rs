"""Safety checks for synthetic identity augmentation."""
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("perf_identity_fixture", ROOT / "scripts/perf_identity_fixture.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class IdentityFixtureTests(unittest.TestCase):
    def test_help_does_not_contact_database(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_identity_fixture.py"), "--help"],
                                env={"MELODEE__DATABASE__URL": "invalid"}, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_nonowned_database_rejected_before_child_process(self):
        with mock.patch.object(sys, "argv", ["perf_identity_fixture.py"]), \
             mock.patch.dict(MODULE.os.environ, {"MELODEE__DATABASE__URL": "postgresql://localhost/production"}, clear=True), \
             mock.patch.object(MODULE.subprocess, "run") as run, contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(MODULE.main(), 1)
            run.assert_not_called()

    def test_augmentation_is_one_transaction_and_refuses_existing_identity_data(self):
        self.assertIn("BEGIN;", MODULE.IDENTITY_SQL)
        self.assertIn("COMMIT;", MODULE.IDENTITY_SQL)
        self.assertIn("identity augmentation requires an untouched fixture", MODULE.IDENTITY_SQL)
        name = "melodee_template_" + "a" * 32
        env = {"MELODEE__DATABASE__URL": "postgresql://localhost/" + name,
               "MELODEE_PERF_OWNED_DATABASE": name}
        with mock.patch.object(sys, "argv", ["perf_identity_fixture.py"]), \
             mock.patch.dict(MODULE.os.environ, env, clear=True), \
             mock.patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            self.assertEqual(MODULE.main(), 0)
            self.assertEqual(run.call_args.kwargs["input"], MODULE.IDENTITY_SQL)
            self.assertIn(env["MELODEE__DATABASE__URL"], run.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
