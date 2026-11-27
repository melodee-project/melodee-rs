"""Safety and attribution checks for the optional syscall profiler."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_media_io_trace as perf


class MediaIoTraceTests(unittest.TestCase):
    def test_help_does_not_spawn_or_contact_services(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_media_io_trace.py"), "--help"],
                                capture_output=True, env={}, check=False)
        self.assertEqual(result.returncode, 0)

    def test_child_environment_does_not_inherit_credentials_or_melodee_settings(self):
        with mock.patch.dict(os.environ, {"PATH": "/bin", "PRIVATE_TOKEN": "secret",
                                         "MELODEE__DATABASE__URL": "production"}, clear=True):
            env = perf.clean_environment(Path("/owned"))
        self.assertEqual(env["PATH"], "/bin")
        self.assertEqual(env["TMPDIR"], "/owned")
        self.assertNotIn("PRIVATE_TOKEN", env)
        self.assertNotIn("MELODEE__DATABASE__URL", env)

    def test_only_release_benchmark_target_is_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            deps = root / "target/release/deps"
            deps.mkdir(parents=True)
            valid = deps / "perf_normalization_matrix-deadbeef"
            valid.write_text("test")
            valid.chmod(0o700)
            unrelated = root / "unrelated"
            unrelated.write_text("test")
            unrelated.chmod(0o700)
            with mock.patch.object(perf, "ROOT", root):
                self.assertEqual(perf.validate_binary(valid), valid)
                with self.assertRaises(ValueError):
                    perf.validate_binary(unrelated)

    def test_parser_sanitizes_paths_and_counts_only_attributed_calls(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, work = root / "source", root / "work"
            trace = root / "trace.1"
            trace.write_text(
                'execve("/usr/bin/ffprobe", ["ffprobe", "-version"], 0x0) = 0\n'
                f'read(3<{source}/track.mp3>, "data", 4) = 4\n'
                f'pwrite64(4<{work}/output.mp3>, "xx", 2, 0) = 2\n'
                f'read(3<{source}/track.mp3>, 0x0, 2) = -1 EIO (Input/output error)\n'
                f'copy_file_range(3<{source}/track.mp3>, NULL, 4<{work}/output.mp3>, NULL, 5, 0) = 5\n'
                f'sendfile(4<{work}/output.mp3>, 3<{source}/track.mp3>, NULL, 3) = 3\n'
                f'splice(3<{source}/track.mp3>, NULL, 4<{work}/output.mp3>, NULL, 2, 0) = 2\n'
                f'mmap(NULL, 4096, PROT_READ, MAP_PRIVATE, 3<{source}/track.mp3>, 0) = 0x123\n'
                'read(5, 0x0, 1) = 0\n'
            )
            result = perf.parse_trace([trace], source, work)
        self.assertEqual(result["executions"], {"ffprobe:version": 1})
        self.assertEqual(result["file_calls"]["ffprobe:source:read"], {"calls": 2, "bytes": 4, "errors": 1})
        self.assertEqual(result["file_calls"]["ffprobe:work:write"]["bytes"], 2)
        self.assertEqual(result["file_calls"]["ffprobe:source:kernel_read"]["bytes"], 10)
        self.assertEqual(result["file_calls"]["ffprobe:work:kernel_write"]["bytes"], 10)
        self.assertEqual(result["attributed_mmaps"], 1)
        self.assertEqual(result["unparsed_file_calls"], 1)
        self.assertNotIn(str(root), json.dumps(result))

    def test_changed_session_topology_never_signals_any_process(self):
        process = mock.Mock(pid=1234)
        process.poll.return_value = None
        with mock.patch.object(perf.os, "getsid", return_value=999), \
             mock.patch.object(perf, "signal_owned_session") as send:
            with self.assertRaisesRegex(RuntimeError, "topology"):
                perf.terminate_owned(process)
        send.assert_not_called()

    def test_pidfds_signal_only_matching_session_and_close_handles(self):
        with mock.patch.object(perf, "session_members", return_value=[42, 41, 40]), \
             mock.patch.object(perf.os, "pidfd_open", side_effect=lambda pid: pid + 100), \
             mock.patch.object(perf.os, "getsid", side_effect=lambda pid: 40 if pid != 41 else 99), \
             mock.patch.object(perf.os, "close") as close, \
             mock.patch.object(perf.signal, "pidfd_send_signal") as send:
            perf.signal_owned_session(40, signal.SIGTERM)
        self.assertEqual(send.call_args_list, [mock.call(142, signal.SIGTERM), mock.call(140, signal.SIGTERM)])
        self.assertEqual(close.call_count, 3)

    @unittest.skipUnless(sys.platform == "linux" and hasattr(os, "pidfd_open")
                         and hasattr(signal, "pidfd_send_signal"), "Linux pidfd required")
    def test_timeout_terminates_owned_descendant_in_its_own_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pid_file = root / "child.pid"
            child = "import os,sys,time; open(sys.argv[1], 'w').write(str(os.getpid())); time.sleep(60)"
            parent = ("import subprocess,sys,time; "
                      "subprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2]],process_group=0); "
                      "time.sleep(60)")
            with self.assertRaises(TimeoutError):
                perf.run_child([sys.executable, "-c", parent, child, str(pid_file)], root,
                               perf.clean_environment(root), 1)
            pid = int(pid_file.read_text())
            status = Path(f"/proc/{pid}/status")
            if status.exists():
                self.assertIn("Z (zombie)", status.read_text())


if __name__ == "__main__":
    unittest.main()
