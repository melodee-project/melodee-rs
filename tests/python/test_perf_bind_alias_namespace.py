"""Safety checks for the test-only bind-alias namespace launcher."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_bind_alias_namespace as launcher


class BindAliasLauncherTests(unittest.TestCase):
    def test_help_does_not_start_children(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_bind_alias_namespace.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_only_exact_reviewed_test_can_be_invoked(self):
        self.assertEqual(launcher.test_command("/test-binary"),
                         ["/test-binary", "custody::tests::private_namespace_bind_aliases_are_rejected",
                          "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        with self.assertRaises(ValueError):
            launcher.test_command("/bin/true")

    def test_test_environment_uses_fixed_trusted_path_and_no_caller_environment(self):
        with mock.patch.dict(os.environ, {"PATH": "/untrusted", "LD_PRELOAD": "/private/secret"}):
            env = launcher.child_environment("mnt:[42]")
        self.assertEqual(env["PATH"], "/usr/bin")
        self.assertEqual(env["TMPDIR"], "/")
        self.assertEqual(env["MELODEE_TEST_ORIGINAL_MOUNT_NS"], "mnt:[42]")
        self.assertNotIn("LD_PRELOAD", env)

    @unittest.skipUnless(Path("/proc/self/fd").is_dir(), "procfs descriptors required")
    def test_fixture_reference_survives_path_replacement_without_touching_external_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original, parked, external = root / "owned", root / "parked", root / "external"
            original.mkdir()
            external.mkdir()
            sentinel = external / "sentinel"
            sentinel.write_text("preserve")
            fd = os.open(original, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                original.rename(parked)
                original.symlink_to(external, target_is_directory=True)
                reference = launcher.fixture_reference(fd)
                (reference / "created").write_text("owned")
                with tempfile.NamedTemporaryFile(dir=reference, delete=False) as log:
                    log_name = Path(log.name).name
                    log.write(b"private log")
                child = subprocess.run([sys.executable, "-c", "from pathlib import Path; Path('child').write_text('owned')"],
                                       cwd=reference, pass_fds=(fd,), check=False)
                self.assertEqual(child.returncode, 0)
                self.assertEqual((parked / "created").read_text(), "owned")
                self.assertEqual((parked / log_name).read_bytes(), b"private log")
                self.assertEqual((parked / "child").read_text(), "owned")
                self.assertFalse((external / "created").exists())
                self.assertFalse((external / log_name).exists())
                self.assertFalse((external / "child").exists())
                self.assertEqual(sentinel.read_text(), "preserve")
            finally:
                os.close(fd)

    def test_replaced_fixture_entry_is_retained_without_deleting_either_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture, held_path = root / "fixture", root / "held"
            fixture.mkdir()
            fd = os.open(fixture, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                self.assertTrue(launcher.fixture_entry_is_original(fixture, fd))
                fixture.rename(held_path)
                fixture.mkdir()
                sentinel = fixture / "sentinel"
                sentinel.write_text("preserve unrelated replacement")
                self.assertFalse(launcher.fixture_entry_is_original(fixture, fd))
                self.assertTrue(held_path.is_dir())
                self.assertEqual(sentinel.read_text(), "preserve unrelated replacement")
            finally:
                os.close(fd)

    def test_context_exit_never_cleans_a_replacement_after_child_window(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with mock.patch.object(launcher, "ROOT", root):
                with launcher.retained_fixture() as name:
                    fixture = Path(name)
                    held = root / "parked"
                    fixture.rename(held)
                    fixture.mkdir()
                    sentinel = fixture / "sentinel"
                    sentinel.write_text("preserve replacement")
                self.assertEqual(sentinel.read_text(), "preserve replacement")
                self.assertTrue(held.is_dir())

    def test_mountinfo_must_be_bounded_unambiguously_private_and_well_formed(self):
        prefix = b"27 19 0:23 / / rw"
        self.assertTrue(launcher.private_mountinfo(prefix + b" - tmpfs tmpfs rw\n"))
        for field in (b"shared:1", b"master:1", b"propagate_from:1", b"unbindable", b"unknown:1"):
            self.assertFalse(launcher.private_mountinfo(prefix + b" " + field + b" - tmpfs tmpfs rw\n"))
        for invalid in (b"", b"malformed", b"a" * (launcher.MAX_MOUNTINFO + 1)):
            self.assertFalse(launcher.private_mountinfo(invalid))

    def test_caller_owned_or_symlinked_tool_directory_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            for name in ("unshare", "mount", "umount"):
                (path / name).write_text("untrusted")
                (path / name).chmod(0o755)
            with self.assertRaises(ValueError):
                launcher.trusted_tool_paths(path)
            link = path.parent / (path.name + "-link")
            link.symlink_to(path, target_is_directory=True)
            try:
                with self.assertRaises(ValueError):
                    launcher.trusted_tool_paths(link)
            finally:
                link.unlink()

    def test_existing_output_invalid_timeout_or_root_never_spawns(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(launcher.subprocess, "Popen") as process:
            root = Path(directory)
            output = root / "output"
            output.write_text("preserved")
            with self.assertRaises(ValueError):
                launcher.run(root / "binary", root / "proof", output, 120)
            with self.assertRaises(ValueError):
                launcher.run(root / "binary", root / "proof", root / "other", 301)
            with mock.patch.object(launcher.os, "geteuid", return_value=0), self.assertRaises(ValueError):
                launcher.run(root / "binary", root / "proof", root / "other", 120)
            process.assert_not_called()
            self.assertEqual(output.read_text(), "preserved")

    @unittest.skipUnless(hasattr(os, "pidfd_open") and hasattr(os, "P_PIDFD"), "Linux pidfd required")
    def test_noisy_child_is_terminated_with_strictly_bounded_capture(self):
        with tempfile.TemporaryDirectory() as directory:
            code = "import os; chunk=b'x'*65536\nwhile True: os.write(1,chunk)"
            returncode, text, censored = launcher.capture_owned(
                [sys.executable, "-c", code], directory, {}, (), 10, limit=1024)
            self.assertTrue(censored)
            self.assertLessEqual(len(text.encode()), 1024)
            self.assertIsInstance(returncode, int)
            self.assertEqual(list(Path(directory).iterdir()), [])

    @unittest.skipUnless(hasattr(os, "pidfd_open") and hasattr(os, "P_PIDFD"), "Linux pidfd required")
    def test_noisy_mount_diagnostic_child_is_censored_and_reaped(self):
        code = "import os; chunk=b'x'*65536\nwhile True: os.write(2,chunk)"
        returncode, text, censored = launcher.capture_mount([sys.executable, "-c", code], (), timeout=10, limit=1024)
        self.assertTrue(censored)
        self.assertLessEqual(len(text.encode()), 1024)
        self.assertIsInstance(returncode, int)

    def test_child_namespace_rejection_precedes_descriptor_or_exec_operations(self):
        args = argparse.Namespace(original_mount="mnt:[1]", original_user="user:[1]", original_pid="pid:[1]", parent_uid=1000, trace_mount=False)
        with mock.patch.object(launcher, "validate_child_namespace", side_effect=ValueError("unsafe")), \
             mock.patch.object(launcher.os, "fstat") as inspect, mock.patch.object(launcher.os, "execve") as execute:
            with self.assertRaises(ValueError):
                launcher.child(args)
            inspect.assert_not_called()
            execute.assert_not_called()

    def test_setup_diagnostics_emit_only_allowlisted_fixed_labels(self):
        data = "\n".join(["bind_alias_stage=proc_mount", "bind_alias_stage=/private/path",
                          "bind_alias_reason=/private/secret", "bind_alias_reason=mount_command_failed",
                          "bind_alias_mount_reason=permission_denied", "bind_alias_mount_reason=/private"])
        self.assertEqual(launcher.sanitized_setup(data), ("proc_mount", "mount_command_failed", "permission_denied", []))
        self.assertEqual(launcher.sanitized_setup("bind_alias_stage=/private"), (None, None, None, []))

    def test_trace_diagnostic_retains_only_allowlisted_syscall_and_errno(self):
        raw = 'mount("private/path", "secret", "proc", 0, NULL) = -1 EINVAL (Invalid argument)\n'
        raw += 'openat("secret") = -1 EACCES\nmove_mount(4, "secret", 5, "secret", 0) = 0\n'
        self.assertEqual(launcher.syscall_labels(raw), ["mount:EINVAL", "move_mount:ok"])
        labels = launcher.sanitized_setup("bind_alias_syscall=mount:EINVAL\nbind_alias_syscall=/private/path")
        self.assertEqual(labels[3], ["mount:EINVAL"])
        self.assertNotIn("secret", repr(labels))

    def test_mount_command_is_gated_on_private_namespace_and_never_canonicalizes_fds(self):
        args = argparse.Namespace(original_mount="mnt:[1]", original_user="user:[1]",
                                  original_pid="pid:[1]", parent_uid=1000, trace_mount=False)
        with mock.patch.object(launcher, "validate_child_namespace", side_effect=ValueError("unsafe")), \
             mock.patch.object(launcher, "capture_mount") as run:
            with self.assertRaises(ValueError):
                launcher.mount_private(args, ["--bind", "/proc/self/fd/5", "/proc/self/fd/6"], [5, 6])
            run.assert_not_called()
        with mock.patch.object(launcher, "validate_child_namespace"), \
             mock.patch.object(launcher, "capture_mount", return_value=(0, "", False)) as run:
            launcher.mount_private(args, ["--bind", "/proc/self/fd/5", "/proc/self/fd/6"], [5, 6])
            self.assertEqual(run.call_args.args[0], ["/usr/bin/mount", "--no-canonicalize", "--bind",
                                                     "/proc/self/fd/5", "/proc/self/fd/6"])
            self.assertEqual(run.call_args.args[1], (5, 6))

    def test_trace_mode_wraps_only_fixed_mount_syscalls(self):
        args = argparse.Namespace(original_mount="mnt:[1]", original_user="user:[1]",
                                  original_pid="pid:[1]", parent_uid=1000, trace_mount=True)
        with mock.patch.object(launcher, "validate_child_namespace"), \
             mock.patch.object(launcher, "capture_mount", return_value=(0, "", False)) as run:
            launcher.mount_private(args, ["--bind", "/proc/self/fd/5", "/proc/self/fd/6"], [5, 6])
            command = run.call_args.args[0]
            self.assertEqual(command[:4], ["/usr/bin/strace", "-f", "-qq", "-e"])
            self.assertEqual(command[4], "trace=" + ",".join(launcher.TRACE_SYSCALLS))
            self.assertEqual(command[5:8], ["/usr/bin/mount", "--no-canonicalize", "--bind"])
            self.assertEqual(run.call_args.args[1], (5, 6))

    def test_wrong_pid_namespace_is_rejected_before_mountinfo(self):
        with mock.patch.object(launcher.os, "geteuid", return_value=0), \
             mock.patch.object(launcher.os, "getpid", return_value=2), \
             mock.patch.object(launcher.os, "readlink", side_effect=["mnt:[2]", "user:[2]", "pid:[2]"]), \
             mock.patch.object(Path, "read_text") as read:
            with self.assertRaises(ValueError):
                launcher.validate_child_namespace("mnt:[1]", "user:[1]", "pid:[1]", 1000)
            read.assert_not_called()

    def test_same_namespace_is_rejected_before_mapping_or_mountinfo(self):
        with mock.patch.object(launcher.os, "geteuid", return_value=0), \
             mock.patch.object(launcher.os, "readlink", return_value="mnt:[42]"), \
             mock.patch.object(Path, "read_text") as read:
            with self.assertRaises(ValueError):
                launcher.validate_child_namespace("mnt:[42]", "user:[1]", "pid:[1]", 1000)
            read.assert_not_called()

    def test_nonroot_namespace_uid_and_zero_parent_uid_are_rejected(self):
        with mock.patch.object(launcher.os, "geteuid", return_value=1000):
            with self.assertRaises(ValueError):
                launcher.validate_child_namespace("mnt:[1]", "user:[1]", "pid:[1]", 1000)
        with mock.patch.object(launcher.os, "geteuid", return_value=0):
            with self.assertRaises(ValueError):
                launcher.validate_child_namespace("mnt:[1]", "user:[1]", "pid:[1]", 0)


if __name__ == "__main__":
    unittest.main()
