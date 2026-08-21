import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from threading import Thread
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace
from unittest.mock import patch
from urllib.parse import urlsplit


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
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_url("https://example.test:invalid/healthz", "url")

    def test_endpoint_fingerprint_does_not_contain_host(self):
        fingerprint = MODULE.endpoint_fingerprint("https://private.example.test/path?q=secret")
        self.assertNotIn("private.example.test", fingerprint)
        self.assertNotIn("secret", fingerprint)
        self.assertEqual(len(fingerprint), 16)

    def test_http_probe_does_not_follow_redirects(self):
        class RedirectHandler(BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802 - BaseHTTPRequestHandler contract
                self.send_response(302)
                self.send_header("Location", "/secret-target")
                self.end_headers()

            def log_message(self, _format, *args):
                return

        server = ThreadingHTTPServer(("127.0.0.1", 0), RedirectHandler)
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            status, _headers, _body = MODULE.http_get(
                f"http://127.0.0.1:{server.server_port}/redirect"
            )
        finally:
            server.shutdown()
            thread.join(timeout=2)
            server.server_close()

        self.assertEqual(status, 302)

    def test_filesystem_probe_rejects_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            target.mkdir()
            link = root / "link"
            link.symlink_to(target, target_is_directory=True)
            with self.assertRaises(MODULE.ProbeError):
                MODULE.filesystem_probe(link, "canonical-root")

            nested = root / "nested"
            nested.mkdir()
            nested_link = nested / "link"
            nested_link.symlink_to(target, target_is_directory=True)
            with self.assertRaises(MODULE.ProbeError):
                MODULE.filesystem_probe(nested_link / "child", "canonical-root")

    def test_backup_probe_rejects_symlinked_parent_component(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            target.mkdir()
            parent_link = root / "backup-parent"
            parent_link.symlink_to(target, target_is_directory=True)
            with self.assertRaises(MODULE.ProbeError):
                MODULE.backup_probe(parent_link / "backup.dump")

    def test_secure_backup_open_rejects_leaf_symlink_and_fifo(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target.dump"
            target.write_bytes(b"fixture")
            link = root / "link.dump"
            link.symlink_to(target)
            with self.assertRaises(MODULE.ProbeError):
                MODULE.open_regular_file_nofollow(link)

            fifo = root / "backup.fifo"
            MODULE.os.mkfifo(fifo)
            with self.assertRaises(MODULE.ProbeError):
                MODULE.open_regular_file_nofollow(fifo)

    def test_storage_topology_uses_held_descriptor_facts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "canonical"
            canonical.mkdir()
            backup = root / "backup.dump"
            backup.write_bytes(b"fixture")
            canonical_facts = MODULE.filesystem_probe(canonical, "canonical-root")
            backup_facts = {
                "bytes": 1,
                "sha256": "b" * 64,
                "device": canonical_facts["device"],
                "toc_entries": 1,
            }
            args = SimpleNamespace(
                base_url="http://127.0.0.1:1",
                range_url=None,
                range_bytes=1,
                oidc_discovery_url=None,
                podcast_feed=None,
                lastfm_api_url=None,
                proxy_url=None,
                require_https=False,
                expect_header=[],
                smtp_host=None,
                smtp_port=587,
                smtp_starttls=False,
                canonical_root=canonical,
                backup_file=backup,
            )
            with patch.object(
                MODULE,
                "http_get",
                return_value=(200, {"Content-Type": "application/json"}, b"{}"),
            ), patch.object(MODULE, "filesystem_probe", return_value=canonical_facts), patch.object(
                MODULE, "backup_probe", return_value=backup_facts
            ), patch.object(
                Path, "stat", side_effect=AssertionError("topology must not restat the path")
            ):
                evidence = MODULE.run(args)
            self.assertTrue(evidence["checks"]["storage_topology"]["same_filesystem"])

    def test_jwks_validation_is_bounded_and_does_not_return_key_material(self):
        summary = MODULE.validate_jwks_document(
            {"keys": [{"kty": "RSA", "kid": "fixture-key", "n": "secret-shaped"}]}
        )
        self.assertEqual(summary, {"key_count": 1, "key_types": ["RSA"]})
        self.assertNotIn("secret-shaped", summary)
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_jwks_document({"keys": [{}]})
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_jwks_document({"keys": [{"kty": "RSA"}] * 257})

    def test_malformed_oidc_and_entity_feed_inputs_fail_as_probe_errors(self):
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_oidc_discovery_document([{"issuer": "https://issuer.example"}])
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_oidc_discovery_document({"issuer": "not-a-url", "jwks_uri": "https://keys.example"})
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_podcast_xml(b"<!DOCTYPE rss [<!ENTITY secret SYSTEM 'file:///private'>]><rss/>")

    def test_oidc_jwks_target_requires_same_host_or_explicit_allowlist(self):
        MODULE.validate_oidc_jwks_target(
            "https://issuer.example/.well-known/openid-configuration",
            "https://issuer.example/keys",
        )
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_oidc_jwks_target(
                "https://issuer.example/.well-known/openid-configuration",
                "https://keys.example/jwks",
            )
        MODULE.validate_oidc_jwks_target(
            "https://issuer.example/.well-known/openid-configuration",
            "https://keys.example/jwks",
            ["keys.example"],
        )

    def test_probe_artifact_validation_is_stable_and_redacted(self):
        artifact = {
            "artifact": "melodee.r4-integration-probe",
            "schema_version": 1,
            "started_at": "2026-08-20T00:00:00Z",
            "base_endpoint_fingerprint": "a" * 16,
            "checks": {
                "/healthz": {"status": 200, "content_type": "application/json"},
                "/readyz": {"status": 200, "content_type": "application/json"},
                "range": {
                    "status": 206,
                    "bytes_read": 7,
                    "content_range": "bytes 0-6/7",
                },
                "oidc_jwks": {"status": 200, "key_count": 1, "key_types": ["RSA"]},
            },
            "result": "pass",
        }
        self.assertIs(MODULE.validate_probe_artifact(artifact), artifact)
        malformed = json.loads(json.dumps(artifact))
        malformed["checks"]["range"]["content_range"] = "bytes 0-7/7"
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_probe_artifact(malformed)
        secret = json.loads(json.dumps(artifact))
        secret["checks"]["/healthz"]["content_type"] = "token=not-redacted"
        with self.assertRaises(MODULE.ProbeError):
            MODULE.validate_probe_artifact(secret)

    def test_proxy_tls_header_assertions_are_bounded_and_value_safe(self):
        self.assertEqual(
            MODULE.parse_expected_header("X-Content-Type-Options=nosniff"),
            ("x-content-type-options", "nosniff"),
        )
        with self.assertRaises(MODULE.ProbeError):
            MODULE.parse_expected_header("bad header=secret")
        with self.assertRaises(MODULE.ProbeError):
            MODULE.parse_expected_header("X-Test=not\nvalid")
        with patch.object(
            MODULE,
            "http_get",
            return_value=(200, {"X-Content-Type-Options": "nosniff"}, b""),
        ):
            evidence = MODULE.proxy_tls_probe(
                "https://proxy.example.test/healthz",
                require_https=True,
                expected_headers=["X-Content-Type-Options=nosniff"],
            )
        self.assertEqual(evidence["status"], 200)
        self.assertEqual(evidence["asserted_headers"], ["x-content-type-options"])
        self.assertNotIn("nosniff", json.dumps(evidence))
        with self.assertRaises(MODULE.ProbeError):
            MODULE.proxy_tls_probe(
                "http://proxy.example.test/healthz",
                require_https=True,
                expected_headers=[],
            )

    def test_run_exercises_bounded_loopback_fixture_checks(self):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802 - BaseHTTPRequestHandler contract
                path = urlsplit(self.path).path
                if path in {"/healthz", "/readyz"}:
                    body = b'{"status":"ok"}'
                    status = 200
                    content_type = "application/json"
                elif path == "/range":
                    body = b"fixture"
                    status = 206
                    self.send_response(status)
                    self.send_header("Content-Range", f"bytes 0-{len(body) - 1}/{len(body)}")
                    self.send_header("Content-Type", "application/octet-stream")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                elif path == "/oidc":
                    body = json.dumps(
                        {
                            "issuer": f"http://127.0.0.1:{self.server.server_port}/issuer",
                            "jwks_uri": f"http://127.0.0.1:{self.server.server_port}/jwks",
                        }
                    ).encode()
                    status = 200
                    content_type = "application/json"
                elif path == "/jwks":
                    body = json.dumps({"keys": [{"kty": "RSA", "kid": "fixture-key"}]}).encode()
                    status = 200
                    content_type = "application/json"
                elif path == "/feed.xml":
                    body = b"<rss version='2.0'><channel><title>fixture</title></channel></rss>"
                    status = 200
                    content_type = "application/rss+xml"
                elif path == "/lastfm":
                    body = b"unauthorized without credentials"
                    status = 401
                    content_type = "text/plain"
                else:
                    body = b"not found"
                    status = 404
                    content_type = "text/plain"
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, _format, *args):
                return

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            base = f"http://127.0.0.1:{server.server_port}"
            evidence = MODULE.run(
                SimpleNamespace(
                    base_url=base,
                    range_url=f"{base}/range",
                    range_bytes=7,
                    oidc_discovery_url=f"{base}/oidc",
                    podcast_feed=f"{base}/feed.xml",
                    lastfm_api_url=f"{base}/lastfm",
                    proxy_url=f"{base}/healthz",
                    require_https=False,
                    expect_header=["Content-Type=application/json"],
                    smtp_host=None,
                    smtp_port=587,
                    smtp_starttls=False,
                    canonical_root=None,
                    backup_file=None,
                )
            )
        finally:
            server.shutdown()
            thread.join(timeout=2)
            server.server_close()

        self.assertEqual(evidence["result"], "pass")
        self.assertEqual(evidence["checks"]["/healthz"]["status"], 200)
        self.assertEqual(evidence["checks"]["/readyz"]["status"], 200)
        self.assertEqual(evidence["checks"]["range"]["status"], 206)
        self.assertEqual(evidence["checks"]["podcast_feed"]["status"], 200)
        self.assertEqual(evidence["checks"]["lastfm_endpoint"]["status"], 401)
        self.assertEqual(evidence["checks"]["proxy_tls"]["status"], 200)
        self.assertEqual(
            evidence["checks"]["proxy_tls"]["asserted_headers"], ["content-type"]
        )
        self.assertEqual(evidence["checks"]["oidc_jwks"]["key_count"], 1)
        self.assertEqual(evidence["checks"]["oidc_jwks"]["key_types"], ["RSA"])
        self.assertNotIn("127.0.0.1", evidence["checks"]["oidc_discovery"]["issuer_fingerprint"])


if __name__ == "__main__":
    unittest.main()
