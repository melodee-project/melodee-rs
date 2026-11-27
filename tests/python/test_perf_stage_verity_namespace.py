"""Containment tests for the private-namespace stage-verity runner."""
from pathlib import Path
import argparse
import json
import os
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_stage_verity_namespace as probe


class StageNamespaceTests(unittest.TestCase):
    def test_sealed_tree_selector_is_exact_and_requires_real_success_marker(self):
        self.assertEqual(probe.test_command("sealed-tree"),
            ["/test-binary", "custody::tests::protected_service_owned_custody_namespace_is_accepted",
             "--exact", "--nocapture", "--test-threads=1"])
        with self.assertRaises(ValueError):
            probe.test_command("custody::tests")
        self.assertEqual(probe.classify_result("sealed-tree", 0, 1, False, False, False, False), "failed")
        self.assertEqual(probe.classify_result("sealed-tree", 0, 1, False, False, True, False), "unsupported")
        self.assertEqual(probe.classify_result("sealed-tree", 101, 0, False, True, False, False), "failed")
        self.assertEqual(probe.classify_result("sealed-tree", 0, 1, False, True, False, False), "verified")
        self.assertEqual(probe.classify_result("sealed-tree", 0, 1, False, True, True, False), "unsupported")

    def test_child_reexec_uses_only_the_readonly_bound_test_binary(self):
        env = probe.test_environment()
        self.assertEqual(env["MELODEE_TEST_EXECUTABLE"], "/test-binary")
        self.assertEqual(set(env), {"PATH", "XDG_RUNTIME_DIR", "HOME", "RUST_TEST_THREADS",
                                    "RUST_BACKTRACE", "MELODEE_TEST_EXECUTABLE"})

    def test_help_does_not_start_processes(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_stage_verity_namespace.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_existing_output_and_invalid_timeout_fail_before_process(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(probe.subprocess, "Popen") as popen:
            out = Path(directory) / "out"
            out.write_text("keep")
            with self.assertRaises(ValueError):
                probe.run(Path("fake"), out, 120)
            with self.assertRaises(ValueError):
                probe.run(Path("fake"), Path(directory) / "other", 301)
            self.assertEqual(out.read_text(), "keep")
            popen.assert_not_called()

    def test_binary_must_be_a_regular_expected_workspace_test(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            fake = target / "melodee_ingestion-0123456789abcdef"
            fake.write_text("fixture")
            bad = target / "other"
            bad.write_text("fixture")
            with mock.patch.object(probe, "TEST_DIR", target):
                self.assertEqual(probe.validate_binary(fake), fake.resolve())
                with self.assertRaises(ValueError):
                    probe.validate_binary(bad)
                link = target / "melodee_ingestion-fedcba9876543210"
                link.symlink_to(fake)
                with self.assertRaises(ValueError):
                    probe.validate_binary(link)

    def test_sealed_tree_requires_build_provenance_before_spawn(self):
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(probe, "validate_binary", return_value=Path(directory) / "binary"), \
             mock.patch.object(probe.subprocess, "Popen") as child:
            with self.assertRaisesRegex(ValueError, "requires a fingerprinted"):
                probe.run(Path("binary"), Path(directory) / "result", 60, "sealed-tree")
            child.assert_not_called()

    def test_panic_locations_are_bounded_and_never_include_paths_or_messages(self):
        text = "\n".join(["thread x panicked at crates/melodee-ingestion/src/custody.rs:42:3: secret /private/path",
                          "thread x panicked at /private/custody.rs:5:1: secret", 
                          "thread x panicked at stage_verity.rs:17:9:"] * 12)
        self.assertEqual(probe.sanitized_panics(text), [
            {"file": "custody.rs", "line": 42, "column": 3},
            {"file": "stage_verity.rs", "line": 17, "column": 9},
        ])
        self.assertEqual(len(probe.sanitized_panics("\n".join(
            f"panicked at custody.rs:{number}:1:" for number in range(20)))), 8)

    def test_mount_failure_rejects_injected_fields_paths_and_invalid_schema(self):
        valid = {"status": "mount_failed", "step": "fixture_bind", "reason": "permission denied", "exit_code": 32}
        self.assertEqual(probe.sanitized_mount_failure(json.dumps(valid)), valid)
        for changed in ({"path": "/private/secret"}, {"reason": "/private/secret"},
                        {"step": ["fixture_bind"]}, {"exit_code": True}, {"exit_code": 999},
                        {"exit_code": 0}):
            injected = dict(valid)
            injected.update(changed)
            with self.subTest(changed=changed):
                self.assertIsNone(probe.sanitized_mount_failure(json.dumps(injected)))
        self.assertIsNone(probe.sanitized_mount_failure('{"status": "mount_failed", bad json'))
        self.assertIsNone(probe.sanitized_mount_failure(json.dumps(valid) + " " * 512))

    def test_private_log_is_exclusive_nofollow_and_restricted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            external = root / "external"
            external.write_bytes(b"preserved")
            link = root / "link"
            link.symlink_to(external)
            with self.assertRaises(OSError):
                probe.write_private_log(link, b"other")
            self.assertEqual(external.read_bytes(), b"preserved")
            out = root / "raw"
            probe.write_private_log(out, b"test")
            self.assertEqual(out.stat().st_mode & 0o777, 0o600)
            self.assertEqual(out.read_bytes(), b"test")
            root.chmod(0o755)
            with self.assertRaises(ValueError):
                probe.write_private_log(root / "rejected", b"test")

    def test_build_proof_matches_all_attested_inputs_and_rejects_tampering(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "binary"
            binary.write_bytes(b"binary")
            proof = root / "proof.json"
            scripts = ("scripts/build_stage_verity_test.py", "scripts/build_perf_server.py",
                       "scripts/perf_stage_verity_namespace.py", "scripts/perf_media_io_trace.py")
            hashes = {name: "script-" + name for name in scripts}
            record = {"target": "sealed-tree-libtest", "profile": "debug", "features": ["test-support"],
                      "binary_name": "binary", "binary_sha256": "binary-digest", "commit": "commit",
                      "source_sha256": {"source": "digest"}, "script_sha256": hashes}
            def digest(path):
                if path == binary:
                    return "binary-digest"
                if path == proof:
                    return "proof-digest"
                return hashes[str(path.relative_to(root))]
            with mock.patch.object(probe, "ROOT", root), mock.patch.object(probe, "file_digest", side_effect=digest), \
                 mock.patch.object(probe, "command_output", return_value="commit"), \
                 mock.patch.object(probe, "source_fingerprints", return_value={"source": "digest"}):
                proof.write_text(json.dumps(record))
                self.assertEqual(probe.validate_build_proof(binary, proof), "proof-digest")
                for key in ("binary_sha256", "commit", "source_sha256", "script_sha256", "features", "profile"):
                    bad = dict(record)
                    bad[key] = "tampered"
                    proof.write_text(json.dumps(bad))
                    with self.subTest(key=key), self.assertRaises(ValueError):
                        probe.validate_build_proof(binary, proof)

    def test_namespace_gate_runs_before_mount_or_filesystem_changes(self):
        args = argparse.Namespace(original_mount_ino=1, original_user_ino=2, original_pid_ino=3, parent_uid=1000)
        with mock.patch.object(probe, "validate_namespace", side_effect=ValueError("unsafe")), \
             mock.patch.object(probe.subprocess, "run") as child:
            with self.assertRaises(ValueError):
                probe.child(args)
            child.assert_not_called()

    def test_existing_pid_or_mount_namespace_is_rejected_before_mount(self):
        current = {name: os.stat("/proc/self/ns/" + name).st_ino for name in ("mnt", "user", "pid")}
        with mock.patch.object(probe.os, "geteuid", return_value=0), \
             mock.patch.object(probe.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "private user, mount and PID"):
                probe.validate_namespace(current["mnt"], current["user"], current["pid"], 1000)
            run.assert_not_called()

    def test_mountinfo_rejects_shared_slave_and_malformed_propagation(self):
        prefix = "27 19 0:23 / / rw"
        self.assertTrue(probe.private_mountinfo(prefix + " - tmpfs tmpfs rw"))
        for propagation in ("shared:1", "master:1", "propagate_from:1"):
            self.assertFalse(probe.private_mountinfo(prefix + " " + propagation + " - tmpfs tmpfs rw"))
        self.assertFalse(probe.private_mountinfo("malformed"))
        self.assertFalse(probe.private_mountinfo(""))

    def test_fixture_fds_close_and_unexpected_inheritable_fd_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            first = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            second = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            extra = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            os.set_inheritable(first, True)
            os.set_inheritable(second, True)
            os.set_inheritable(extra, True)
            try:
                with self.assertRaisesRegex(ValueError, "unexpected inheritable"):
                    probe.close_fixture_fds(first, second)
                with self.assertRaises(OSError):
                    os.fstat(first)
                with self.assertRaises(OSError):
                    os.fstat(second)
                os.fstat(extra)
            finally:
                os.close(extra)

    def test_fixture_parent_must_be_private_owned_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            probe.private_directory(parent)
            parent.chmod(0o755)
            with self.assertRaises(ValueError):
                probe.private_directory(parent)


if __name__ == "__main__":
    unittest.main()
