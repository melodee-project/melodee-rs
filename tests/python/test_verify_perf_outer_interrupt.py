"""Validation for the real outer-fixture interruption test."""
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import verify_perf_outer_interrupt as check


class OuterInterruptTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/verify_perf_outer_interrupt.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_requires_owned_parent_and_all_cleanup_evidence(self):
        name = "melodee_template_" + "a" * 32
        record = {"status": "passed", "signal": "SIGTERM", "signal_target": "fixture",
                  "parent_fixture": name, "clone_removed": True, "server_reaped": True,
                  "source_intact": True}
        self.assertEqual(check.check_result(1, json.dumps(record), "SIGTERM"), name)
        for change in ({"parent_fixture": "production"}, {"clone_removed": False},
                       {"server_reaped": False}, {"signal": "SIGINT"}):
            with self.assertRaises(RuntimeError):
                check.check_result(1, json.dumps({**record, **change}), "SIGTERM")
        with self.assertRaises(RuntimeError):
            check.check_result(0, json.dumps(record), "SIGTERM")
        with self.assertRaises(RuntimeError):
            check.check_result(1, json.dumps(record) + "\n" + json.dumps(record), "SIGTERM")

    def test_only_the_verified_owned_session_is_signalled_and_reaped(self):
        child = mock.Mock(pid=1234)
        child.poll.return_value = None
        with mock.patch.object(check.os, "getpgid", return_value=1234), \
             mock.patch.object(check.os, "getsid", return_value=1234), \
             mock.patch.object(check.os, "killpg") as kill:
            check.stop_fixture(child)
        kill.assert_called_once_with(1234, check.signal.SIGTERM)
        child.wait.assert_called_once_with(timeout=45)

    def test_changed_session_is_never_signalled(self):
        child = mock.Mock(pid=1234)
        child.poll.return_value = None
        with mock.patch.object(check.os, "getpgid", return_value=99), \
             mock.patch.object(check.os, "getsid", return_value=1234), \
             mock.patch.object(check.os, "killpg") as kill:
            with self.assertRaises(RuntimeError):
                check.stop_fixture(child)
        kill.assert_not_called()


if __name__ == "__main__":
    unittest.main()
