from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "do-prechecks.py"
spec = importlib.util.spec_from_file_location("do_prechecks", SCRIPT)
assert spec is not None and spec.loader is not None
prechecks = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = prechecks
spec.loader.exec_module(prechecks)


class PrecheckTests(unittest.TestCase):
    def test_default_checks_include_both_diffs_full_gate_and_docs(self) -> None:
        self.assertEqual(
            [check.command for check in prechecks.checks_for()],
            [
                ("git", "diff", "--check"),
                ("git", "diff", "--cached", "--check"),
                (sys.executable, "-u", "scripts/check.py", "full"),
                (sys.executable, "-u", "scripts/check.py", "docs"),
            ],
        )
        checks = prechecks.checks_for(include_e2e=True)
        self.assertEqual(checks[:-1], prechecks.checks_for())
        self.assertEqual(checks[-1].command[-1], "admin-ui-e2e")

    def test_missing_optional_full_gate_tools_fail_before_any_commands(self) -> None:
        for tool in ("pnpm", "cargo-deny", "ffmpeg", "ffprobe", "fpcalc"):
            with (
                self.subTest(tool=tool),
                mock.patch.object(
                    prechecks.shutil, "which",
                    side_effect=lambda name: None if name == tool else name,
                ),
                mock.patch.object(prechecks.subprocess, "run") as run,
                contextlib.redirect_stderr(io.StringIO()) as output,
            ):
                self.assertEqual(prechecks.main([]), 2)
                self.assertIn(tool, output.getvalue())
                run.assert_not_called()

    def test_help_runs_without_tools_from_another_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--help"],
                cwd=directory,
                env={**os.environ, "PATH": ""},
                capture_output=True,
                text=True,
                check=False,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--include-e2e", result.stdout)

    def test_fail_fast_preserves_status_and_reports_unrun_checks(self) -> None:
        checks = prechecks.checks_for()
        for failure_index in range(len(checks)):
            with (
                self.subTest(failure_index=failure_index),
                mock.patch.object(
                    prechecks.subprocess, "run",
                    side_effect=[subprocess.CompletedProcess([], 0)] * failure_index
                    + [subprocess.CompletedProcess([], 7)],
                ) as run,
                contextlib.redirect_stdout(io.StringIO()) as stdout,
                contextlib.redirect_stderr(io.StringIO()) as stderr,
            ):
                self.assertEqual(prechecks.run_checks(checks), 7)
                self.assertEqual(run.call_count, failure_index + 1)
                self.assertNotIn("selected prechecks passed", stdout.getvalue())
                self.assertIn(checks[failure_index].name, stderr.getvalue())
                for check in checks[failure_index + 1:]:
                    self.assertIn(check.name, stderr.getvalue())

    def test_success_uses_repository_cwd_and_reports_all_selected_checks(self) -> None:
        with (
            mock.patch.object(prechecks, "validate_environment"),
            mock.patch.object(
                prechecks.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)
            ) as run,
            contextlib.redirect_stdout(io.StringIO()) as output,
        ):
            self.assertEqual(prechecks.main(["--include-e2e"]), 0)
        self.assertEqual(run.call_count, 5)
        for call in run.call_args_list:
            self.assertEqual(call.kwargs, {"cwd": ROOT, "shell": False, "check": False})
        self.assertIn("All 5 selected prechecks passed", output.getvalue())

    def test_launch_failure_and_interrupt_return_useful_status(self) -> None:
        for error, status in ((OSError("cannot start"), 2), (KeyboardInterrupt(), 130)):
            with (
                self.subTest(status=status),
                mock.patch.object(prechecks, "validate_environment"),
                mock.patch.object(prechecks.subprocess, "run", side_effect=error),
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(prechecks.main([]), status)

    def test_signalled_child_is_a_failure(self) -> None:
        with (
            mock.patch.object(
                prechecks.subprocess, "run", return_value=subprocess.CompletedProcess([], -15)
            ),
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            self.assertEqual(prechecks.run_checks(prechecks.checks_for()), 143)

    def test_real_child_failure_prevents_later_side_effects(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "must-not-exist"
            checks = [
                prechecks.Check("Failure", (sys.executable, "-c", "raise SystemExit(9)")),
                prechecks.Check(
                    "Must not run",
                    (sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).touch()"),
                ),
            ]
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(prechecks.run_checks(checks), 9)
            self.assertFalse(marker.exists())


if __name__ == "__main__":
    unittest.main()
