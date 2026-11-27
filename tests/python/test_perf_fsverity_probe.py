"""Safety and ABI checks for the isolated fs-verity experiment."""
import errno
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import perf_fsverity_probe as probe


class FsVerityProbeTests(unittest.TestCase):
    def test_help_has_no_file_mutation(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/perf_fsverity_probe.py"), "--help"],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_abi_struct_and_ioctl_match_linux_x86_64(self):
        self.assertEqual(struct.calcsize(probe.ARG_FORMAT), 128)
        self.assertEqual(probe.FS_IOC_ENABLE_VERITY, (1 << 30) | (128 << 16) | (ord('f') << 8) | 133)

    @unittest.skipUnless(sys.platform == "linux", "Linux mmap fixture")
    def test_mocked_success_creates_only_owned_file_and_cleans_up(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            untouched = parent / "neighbor"
            untouched.write_bytes(b"keep")
            with mock.patch.object(probe, "enable", side_effect=[errno.ENOTTY, errno.ETXTBSY, errno.ETXTBSY, 0]), \
                 mock.patch.object(probe, "write_attempt", return_value=errno.EPERM):
                result = probe.probe(parent)
            self.assertEqual(result["status"], "verified")
            self.assertEqual(untouched.read_bytes(), b"keep")
            self.assertEqual([entry.name for entry in parent.iterdir()], ["neighbor"])

    @unittest.skipUnless(sys.platform == "linux", "Linux fixture")
    def test_unsupported_filesystem_is_not_reported_as_verified(self):
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(probe, "enable", side_effect=[errno.ENOTTY, errno.EOPNOTSUPP]):
            result = probe.probe(Path(directory))
            self.assertEqual(result["status"], "unsupported")
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_unsupported_platform_fails_before_ioctl(self):
        with mock.patch.object(probe.platform, "machine", return_value="unsupported"), \
             mock.patch.object(probe, "enable") as ioctl:
            with self.assertRaises(ValueError):
                probe.probe(ROOT)
            ioctl.assert_not_called()


if __name__ == "__main__":
    unittest.main()
