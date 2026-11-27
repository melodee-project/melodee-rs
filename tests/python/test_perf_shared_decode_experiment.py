"""Safety and result parsing for the test-only shared-decode experiment."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_shared_decode_experiment as experiment


class SharedDecodeExperimentTests(unittest.TestCase):
    def test_help_does_not_start_children(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_shared_decode_experiment.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_unavailable_muxer_falls_back_without_constructing_candidate(self):
        self.assertIsNone(experiment.candidate_command(Path("source"), Path("output"), False))
        command = experiment.candidate_command(Path("source"), Path("output"), True)
        self.assertIn("-xerror", command)
        self.assertIn("-n", command)
        self.assertIn("-algorithm", command)
        self.assertEqual(command[command.index("-algorithm") + 1], "1")

    def test_fpcalc_duration_and_probe_duration_are_compared_explicitly(self):
        self.assertEqual(experiment.milliseconds("1.2345"), 1235)
        self.assertIsNone(experiment.milliseconds("NaN"))
        self.assertIsNone(experiment.milliseconds(-1))
        self.assertEqual(experiment.fingerprint('{"duration":2.5,"fingerprint":"abc"}'), ("abc", 2.5))
        self.assertEqual(experiment.fingerprint("invalid"), (None, None))
        self.assertEqual(experiment.ffprobe_duration('{"streams":[{"codec_type":"audio","duration":"1.5"}],"format":{"duration":"1.6"}}'), "1.5")

    def test_resampling_precedes_exact_sample_cutoff(self):
        command = experiment.candidate_command(Path("source"), Path("output"), True,
                                                variant="resample-then-samples")
        graph = command[command.index("-filter_complex") + 1]
        self.assertIn("aresample=11025:osf=s16:ochl=mono,atrim=end_sample=1323000", graph)
        compatible = experiment.candidate_command(Path("source"), Path("output"), True,
                                                   variant="fpcalc-compatible-resampler")
        graph = compatible[compatible.index("-filter_complex") + 1]
        self.assertIn("filter_size=16:phase_shift=8:linear_interp=1:cutoff=0.8", graph)
        self.assertIn("atrim=end_sample=1323000", graph)
        sample_command = experiment.candidate_command(Path("source"), Path("output"), True,
                variant="fpcalc-compatible-resampler", sample_output=Path("samples"))
        self.assertIn("[samplecheck]", sample_command)
        self.assertIn("samples", sample_command)

    def test_fpcalc_like_duration_uses_stream_ticks_and_display_rounding(self):
        probe = '{"streams":[{"codec_type":"audio","duration_ts":720312,"time_base":"1/48000"}]}'
        self.assertEqual(experiment.fpcalc_like_duration(probe), 15010)
        fallback = '{"streams":[{"codec_type":"audio"}],"format":{"duration":"15.006500"}}'
        self.assertEqual(experiment.fpcalc_like_duration(fallback), 15010)
        self.assertIsNone(experiment.fpcalc_like_duration('{"streams":[]}'))
        self.assertIsNone(experiment.fpcalc_like_duration('{"streams":[{"codec_type":"audio"}],"format":{"duration":"N/A"}}'))
        self.assertIsNone(experiment.fpcalc_like_duration('{"streams":[{"codec_type":"audio","duration_ts":5,"time_base":"1/0"}]}'))

    @unittest.skipUnless(sys.platform == "linux" and hasattr(os, "pidfd_open")
                         and hasattr(signal, "pidfd_send_signal"), "Linux pidfd required")
    def test_timeout_terminates_descendant_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pid_file = root / "child.pid"
            child = "import os,sys,time; open(sys.argv[1], 'w').write(str(os.getpid())); time.sleep(60)"
            parent = ("import subprocess,sys,time; "
                      "subprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2]],process_group=0); "
                      "time.sleep(60)")
            result = experiment.run_command([sys.executable, "-c", parent, child, str(pid_file)],
                                            root, experiment.clean_environment(root), 1)
            self.assertTrue(result["timed_out"])
            pid = int(pid_file.read_text())
            status = Path(f"/proc/{pid}/status")
            if status.exists():
                self.assertIn("Z (zombie)", status.read_text())


if __name__ == "__main__":
    unittest.main()
