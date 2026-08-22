"""Safety checks for the bounded release-image smoke harness."""
from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "container_smoke.py"
RELEASE_WORKFLOW = ROOT / ".github" / "workflows" / "release-container.yml"


def load_module():
    spec = importlib.util.spec_from_file_location("container_smoke", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["container_smoke"] = module
    spec.loader.exec_module(module)
    return module


class ContainerSmokeInputTests(unittest.TestCase):
    def run_script(self, **updates: str) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment.update(
            {
                "SMOKE_IMAGE": "synthetic/melodee:local",
                "RELEASE_VERSION": "0.1.0",
                "SMOKE_RUN_ID": "python-test",
                "SMOKE_PORT": "18099",
            }
        )
        environment.update(updates)
        return subprocess.run(
            [sys.executable, str(SCRIPT)],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_invalid_run_id_fails_before_docker_access(self) -> None:
        result = self.run_script(SMOKE_RUN_ID="bad/name")
        self.assertEqual(result.returncode, 2)
        self.assertIn("unsupported characters", result.stderr)
        self.assertNotIn("docker", result.stderr.lower())

    def test_invalid_port_fails_closed(self) -> None:
        result = self.run_script(SMOKE_PORT="80")
        self.assertEqual(result.returncode, 2)
        self.assertIn("between 1024 and 65535", result.stderr)

    def test_script_is_importable_python(self) -> None:
        module = load_module()
        self.assertTrue(callable(module.main))

    def test_help_is_available_without_release_environment(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--help"],
            cwd=ROOT,
            env={"PATH": os.environ["PATH"]},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("usage:", result.stdout.lower())
        self.assertIn("SMOKE_IMAGE", result.stdout)
        self.assertNotIn("docker", result.stderr.lower())

    def test_script_verifies_release_labels(self) -> None:
        module = load_module()
        labels = {
            "org.opencontainers.image.title": "Melodee",
            "org.opencontainers.image.source": "https://github.com/melodee-project/melodee-rs",
            "org.opencontainers.image.version": "0.1.0",
            "org.opencontainers.image.licenses": "MIT",
            "org.opencontainers.image.revision": "abc123",
        }
        module.validate_labels(json.dumps(labels), "0.1.0", "abc123")
        labels["org.opencontainers.image.licenses"] = "proprietary"
        with self.assertRaises(module.SmokeError):
            module.validate_labels(json.dumps(labels), "0.1.0", "abc123")


class ReleaseWorkflowTests(unittest.TestCase):
    def test_prepublication_gate_builds_and_scans_both_architectures(self) -> None:
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("architecture: amd64", workflow)
        self.assertIn("architecture: arm64", workflow)
        self.assertIn("platform: linux/amd64", workflow)
        self.assertIn("platform: linux/arm64", workflow)
        self.assertIn("platforms: ${{ matrix.platform }}", workflow)
        self.assertIn("if: matrix.architecture == 'arm64'", workflow)
        self.assertIn(
            "image-ref: melodee-release-smoke:${{ github.run_id }}-${{ matrix.architecture }}",
            workflow,
        )
        self.assertIn("anchore/sbom-action/download-syft@v0", workflow)
        self.assertIn("syft-version: v1.51.0", workflow)
        self.assertIn("format: json", workflow)
        self.assertIn("output: trivy-critical.json", workflow)
        self.assertIn("output: trivy-high.json", workflow)
        self.assertIn("sha256sum --", workflow)
        self.assertIn("name: Require complete image evidence", workflow)
        for evidence_file in (
            "image-metadata.json",
            "image-digest.txt",
            "sbom.spdx.json",
            "trivy-critical.json",
            "trivy-high.json",
            "scan-status.txt",
            "release-evidence.json",
            "SHA256SUMS",
        ):
            self.assertIn(evidence_file, workflow)
        self.assertIn("scripts/validate_release_evidence.py", workflow)
        self.assertIn("uses: actions/upload-artifact@v4", workflow)
        self.assertIn("if-no-files-found: error", workflow)
        self.assertIn("if: always()", workflow)
        self.assertIn("name: Archive exact scanned image", workflow)
        self.assertIn('docker save "$SMOKE_IMAGE" | gzip -1', workflow)
        self.assertIn("image.tar.gz", workflow)

    def test_publication_waits_for_the_complete_architecture_matrix(self) -> None:
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.assertIn(
            "needs: [validate-release, quality-gate, build-and-test]",
            workflow,
        )
        self.assertIn("uses: actions/download-artifact@v4", workflow)
        self.assertIn("docker load --input", workflow)
        self.assertIn("staging-${{ github.run_id }}", workflow)
        self.assertIn("docker buildx imagetools create", workflow)
        self.assertNotIn("name: Build and publish multi-platform release image", workflow)

    def test_release_quality_gate_requires_terminal_requirement_dispositions(self) -> None:
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("name: Require terminal requirement dispositions", workflow)
        self.assertIn(
            "python3 scripts/audit_requirement_overrides.py --require-terminal",
            workflow,
        )

    def test_workflow_invokes_python_smoke_harness(self) -> None:
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("python3 scripts/container_smoke.py", workflow)
        self.assertNotIn("scripts/container_smoke.sh", workflow)


if __name__ == "__main__":
    unittest.main()
