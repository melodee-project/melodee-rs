from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


class QualifyLastfmWireTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("qualify_lastfm_wire", ROOT / "scripts" / "qualify_lastfm_wire.py")

    def test_signature_matches_rust_contract(self) -> None:
        # Sorted name+value pairs, secret last, MD5 hex; format/api_sig excluded.
        params = {
            "format": "json",
            "method": "auth.getSession",
            "token": "abc",
            "api_key": "key",
        }
        signature = self.module.build_api_signature(params, "secret")
        expected = self.module.build_api_signature(
            {"method": "auth.getSession", "token": "abc", "api_key": "key"}, "secret"
        )
        self.assertEqual(signature, expected)
        self.assertEqual(len(signature), 32)
        self.assertTrue(all(c in "0123456789abcdef" for c in signature))

    def test_clear_session_error_codes(self) -> None:
        self.assertEqual(self.module.CLEAR_SESSION_ERROR_CODES, {4, 9, 10})

    def test_error_parsing_json_and_xml(self) -> None:
        code, message = self.module.parse_lfm_error('{"error": 10, "message": "Invalid API key"}')
        self.assertEqual((code, message), (10, "Invalid API key"))
        code, message = self.module.parse_lfm_error(
            '<lfm status="failed"><error code="4">Authentication Failed</error></lfm>'
        )
        self.assertEqual((code, message), (4, "Authentication Failed"))
        code, message = self.module.parse_lfm_error('{"session": {"key": "k"}}')
        self.assertEqual((code, message), (None, None))

    def test_extract_credentials_requires_both_values(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            csv_path = Path(tmp) / "settings.csv"
            csv_path.write_text(
                "Key,Value\n"
                "scrobbling.lastFm.apiKey,key-abc\n"
                "scrobbling.lastFm.sharedSecret,secret-xyz\n"
                "unrelated.key,1\n",
                encoding="utf-8",
            )
            api_key, shared_secret = self.module.extract_credentials(csv_path)
            self.assertEqual((api_key, shared_secret), ("key-abc", "secret-xyz"))
            missing = Path(tmp) / "missing.csv"
            missing.write_text("Key,Value\nscrobbling.lastFm.apiKey,key-abc\n", encoding="utf-8")
            with self.assertRaises(SystemExit):
                self.module.extract_credentials(missing)

    def test_redaction_removes_credential_fields(self) -> None:
        observation = {
            "status": "ok",
            "http_status": 200,
            "body": "irrelevant-public-response",
            "api_key": "key-abc",
            "session_key": "leak",
        }
        cleaned = self.module.redact_observation(observation)
        self.assertNotIn("api_key", cleaned)
        self.assertNotIn("session_key", cleaned)
        self.assertEqual(cleaned["body"], "irrelevant-public-response")

    def test_main_refuses_to_persist_credentials(self) -> None:
        # Offline-deterministic: canned observations stand in for real network
        # probes; the credential-guard must refuse a leak regardless.
        canned = [
            {"status": "ok", "http_status": 200, "latency_ms": 5, "body": '{"error": 10, "message": "Invalid API key"}'},
            {"status": "ok", "http_status": 200, "latency_ms": 5, "body": '{"error": 10, "message": "Invalid token"}'},
            {"status": "ok", "http_status": 200, "latency_ms": 5, "body": '{"error": 4, "message": "Authentication Failed"}'},
            {"status": "ok", "http_status": 200, "latency_ms": 5, "body": '{"error": 13, "message": "Invalid method signature supplied"}'},
            {"status": "transport_error", "transport_reason": "URLError", "latency_ms": 0},
        ]
        with tempfile.TemporaryDirectory() as tmp:
            csv_path = Path(tmp) / "settings.csv"
            csv_path.write_text(
                "Key,Value\n"
                "scrobbling.lastFm.apiKey,secret-key-material\n"
                "scrobbling.lastFm.sharedSecret,secret-secret-material\n",
                encoding="utf-8",
            )
            output = Path(tmp) / "evidence.json"
            original = self.module.call_lastfm
            self.module.call_lastfm = lambda *args, **kwargs: canned.pop(0)
            try:
                exit_code = self.module.main(["--csv", str(csv_path), "--output", str(output)])
            finally:
                self.module.call_lastfm = original
            self.assertEqual(exit_code, 0)
            rendered = output.read_text(encoding="utf-8")
            self.assertNotIn("secret-key-material", rendered)
            self.assertNotIn("secret-secret-material", rendered)
            document = json.loads(rendered)
            self.assertEqual(document["format"], "melodee-lastfm-wire-evidence/v1")
            self.assertIn("unexercised", document)
            self.assertEqual(len(document["probes"]), 5)
            # Invalid-token link attempt and bogus-session scrobble must both
            # classify as clear-session triggers against the real contract.
            self.assertTrue(document["probes"][2]["clear_session_trigger"])
            self.assertTrue(document["probes"][1]["clear_session_trigger"])
            self.assertEqual(document["probes"][-1]["status"], "transport_error")

    def test_help_is_safe(self) -> None:
        with self.assertRaises(SystemExit) as caught:
            self.module.main(["--help"])
        self.assertEqual(caught.exception.code, 0)


if __name__ == "__main__":
    unittest.main()
