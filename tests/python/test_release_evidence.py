"""Tests for the release-image evidence bundle validator."""
from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "validate_release_evidence.py"


class ReleaseEvidenceTests(unittest.TestCase):
    def make_bundle(self) -> Path:
        directory = Path(tempfile.mkdtemp(prefix="melodee-release-evidence-"))
        image_id = "sha256:" + "a" * 64
        labels = {
            "org.opencontainers.image.title": "Melodee",
            "org.opencontainers.image.source": "https://github.com/melodee-project/melodee-rs",
            "org.opencontainers.image.licenses": "MIT",
            "org.opencontainers.image.version": "0.1.0",
            "org.opencontainers.image.revision": "abc123",
        }
        (directory / "image-metadata.json").write_text(
            json.dumps(
                {
                    "Id": image_id,
                    "Architecture": "amd64",
                    "Os": "linux",
                    "Config": {"User": "melodee:melodee", "Labels": labels},
                }
            ),
            encoding="utf-8",
        )
        (directory / "image-digest.txt").write_text(image_id + "\n", encoding="utf-8")
        (directory / "image.tar.gz").write_bytes(b"exact-scanned-image-archive")
        (directory / "sbom.spdx.json").write_text(
            json.dumps({"spdxVersion": "SPDX-2.3", "packages": []}), encoding="utf-8"
        )
        for name in ("trivy-critical.json", "trivy-high.json"):
            (directory / name).write_text(json.dumps({"SchemaVersion": 2, "Results": []}), encoding="utf-8")
        (directory / "scan-status.txt").write_text(
            "architecture=amd64\ncritical_scan=present\nhigh_scan=present\n", encoding="utf-8"
        )
        return directory

    def run_validator(self, directory: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["python3", str(SCRIPT), "--directory", str(directory), *arguments],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_write_then_checksum_and_validate_bundle(self) -> None:
        directory = self.make_bundle()
        written = self.run_validator(
            directory,
            "--architecture",
            "amd64",
            "--version",
            "0.1.0",
            "--revision",
            "abc123",
            "--write",
        )
        self.assertEqual(written.returncode, 0, written.stderr)
        files = sorted(path.name for path in directory.iterdir() if path.is_file())
        checksum_lines = []
        for name in files:
            checksum = hashlib.sha256((directory / name).read_bytes()).hexdigest()
            checksum_lines.append(f"{checksum}  {name}")
        (directory / "SHA256SUMS").write_text("\n".join(checksum_lines) + "\n", encoding="utf-8")
        checked = self.run_validator(
            directory,
            "--architecture",
            "amd64",
            "--version",
            "0.1.0",
            "--revision",
            "abc123",
            "--check",
        )
        self.assertEqual(checked.returncode, 0, checked.stderr)
        manifest = json.loads((directory / "release-evidence.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["schemaVersion"], 1)
        self.assertEqual(manifest["architecture"], "amd64")

    def test_write_rejects_root_runtime_user(self) -> None:
        directory = self.make_bundle()
        metadata = json.loads((directory / "image-metadata.json").read_text(encoding="utf-8"))
        metadata["Config"]["User"] = "root"
        (directory / "image-metadata.json").write_text(json.dumps(metadata), encoding="utf-8")
        result = self.run_validator(
            directory,
            "--architecture",
            "amd64",
            "--version",
            "0.1.0",
            "--revision",
            "abc123",
            "--write",
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("melodee:melodee", result.stderr)

    def test_check_rejects_tampered_report(self) -> None:
        directory = self.make_bundle()
        written = self.run_validator(
            directory,
            "--architecture",
            "amd64",
            "--version",
            "0.1.0",
            "--revision",
            "abc123",
            "--write",
        )
        self.assertEqual(written.returncode, 0, written.stderr)
        (directory / "trivy-high.json").write_text("{}", encoding="utf-8")
        self.assertEqual(
            self.run_validator(directory, "--architecture", "amd64", "--check").returncode,
            2,
        )

    def test_check_rejects_symlinked_evidence_file(self) -> None:
        directory = self.make_bundle()
        written = self.run_validator(
            directory,
            "--architecture",
            "amd64",
            "--version",
            "0.1.0",
            "--revision",
            "abc123",
            "--write",
        )
        self.assertEqual(written.returncode, 0, written.stderr)
        target = directory / "outside-report.json"
        target.write_text("{}", encoding="utf-8")
        report = directory / "trivy-high.json"
        report.unlink()
        report.symlink_to(target)
        self.assertEqual(
            self.run_validator(directory, "--architecture", "amd64", "--check").returncode,
            2,
        )

    def test_check_rejects_symlinked_evidence_directory(self) -> None:
        real_directory = self.make_bundle()
        written = self.run_validator(
            real_directory,
            "--architecture",
            "amd64",
            "--version",
            "0.1.0",
            "--revision",
            "abc123",
            "--write",
        )
        self.assertEqual(written.returncode, 0, written.stderr)
        link_directory = real_directory.parent / (real_directory.name + "-link")
        link_directory.symlink_to(real_directory, target_is_directory=True)
        self.assertEqual(
            self.run_validator(link_directory, "--architecture", "amd64", "--check").returncode,
            2,
        )


if __name__ == "__main__":
    unittest.main()
