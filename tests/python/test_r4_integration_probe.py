import importlib.util
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "r4_integration_probe", ROOT / "scripts" / "r4_integration_probe.py"
)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class R4IntegrationProbeTests(unittest.TestCase):
    def test_url_validation_rejects_embedded_credentials(self):
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_url("https://user:secret@example.test", "url")

    def test_endpoint_fingerprint_does_not_contain_host(self):
        fingerprint = MODULE.endpoint_fingerprint("https://private.example.test/path?q=secret")
        self.assertNotIn("private.example.test", fingerprint)
        self.assertNotIn("secret", fingerprint)
        self.assertEqual(len(fingerprint), 16)

    def test_filesystem_probe_rejects_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            target.mkdir()
            link = root / "link"
            link.symlink_to(target, target_is_directory=True)
            with self.assertRaises(MODULE.ProbeError):
                MODULE.filesystem_probe(link, "canonical-root")


if __name__ == "__main__":
    unittest.main()

