"""Tests for Issue 5 native API v1 capture tooling and golden fixtures.

Covers: oracle fixture determinism, capture-time sanitization (no credential
material may persist), token structure validators, the unified route
inventory, and the committed golden evidence itself.
"""
from __future__ import annotations

import base64
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GOLDEN = ROOT / "compatibility" / "melodee-api-v1" / "golden"

sys.path.insert(0, str(ROOT / "tests" / "python"))
from test_scripts import load_module  # noqa: E402

capture_baseline = load_module("capture_baseline", ROOT / "scripts" / "capture_baseline.py")
legacy_oracle = load_module("legacy_oracle", ROOT / "scripts" / "legacy_oracle.py")
build_native_inventory = load_module(
    "build_native_inventory", ROOT / "scripts" / "build_native_inventory.py"
)
gen_fixture_tokens = load_module("gen_fixture_tokens", ROOT / "scripts" / "gen_fixture_tokens.py")


class FixtureDeterminismTests(unittest.TestCase):
    def test_fixture_uuid_is_stable(self) -> None:
        self.assertEqual(
            legacy_oracle.fixture_uuid("user", "phase0-fixture"),
            "f78448cc-82ec-5864-877c-6c6163b00065",
        )
        self.assertEqual(
            legacy_oracle.fixture_uuid("artist", "aurora-fields"),
            "813c11e0-61cd-5204-ae0f-ce68386eacec",
        )

    def test_fixture_credentials_are_documented_synthetics(self) -> None:
        for value in (
            legacy_oracle.FIXTURE_JWT_KEY,
            legacy_oracle.FIXTURE_AUTH_TOKEN,
            legacy_oracle.FIXTURE_SECURITY_KEY,
        ):
            self.assertIn("fixture", value.lower())
            self.assertIn("NOT-A-REAL-SECRET", value)

    def test_seed_sql_contains_no_placeholder_baseurl(self) -> None:
        catalog = json.loads(
            (ROOT / "compatibility" / "fixtures" / "catalog-v1.json").read_text(encoding="utf-8")
        )
        media = {
            song["slug"]: (100, "0" * 64)
            for artist in catalog["artists"]
            for album in artist["albums"]
            for song in album["songs"]
        }
        sql = legacy_oracle.seed_sql(catalog, media)
        self.assertNotIn("REQUIRED: THIS MUST BE EDITED", sql)
        # Deterministic public IDs must be pinned in the seed.
        self.assertIn("813c11e0-61cd-5204-ae0f-ce68386eacec", sql)
        # The name-collision fixture pair must share a normalized name.
        self.assertIn("'Dune'", sql)
        self.assertIn("'Düné'", sql)

    def test_wav_generation_is_deterministic(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            first = Path(tmp) / "a.wav"
            second = Path(tmp) / "b.wav"
            legacy_oracle.make_wav(first, seconds=0.1, frequency=220.0)
            legacy_oracle.make_wav(second, seconds=0.1, frequency=220.0)
            self.assertEqual(first.read_bytes(), second.read_bytes())


class SanitizationTests(unittest.TestCase):
    def test_stream_auth_token_masked_in_urls(self) -> None:
        url = (
            "http://127.0.0.1:5157/song/stream/2b8eab4b-235d-5892-99f8-c0f5f54e8ab4/"
            "f78448cc-82ec-5864-877c-6c6163b00065/Zm9vYmFyMTIzfDEyMzR8aGFoYQ"
        )
        redacted = capture_baseline.redact_url(url)
        self.assertIn("<hmac-auth-token>", redacted)
        self.assertNotIn("Zm9vYmFyMTIzfDEyMzR8aGFoYQ", redacted)
        self.assertIn("2b8eab4b-235d-5892-99f8-c0f5f54e8ab4", redacted)

    def test_sensitive_query_keys_still_redacted(self) -> None:
        url = "http://example.test/song/stream/a/b/c?jwt=secret-token&u=alice"
        redacted = capture_baseline.redact_url(url)
        self.assertNotIn("secret-token", redacted)
        self.assertIn("u=alice", redacted)

    def test_mask_stream_auth_tokens_inside_json(self) -> None:
        document = {
            "data": [
                {
                    "streamUrl": "http://h/song/stream/2b8eab4b-235d-5892-99f8-c0f5f54e8ab4/"
                    "f78448cc-82ec-5864-877c-6c6163b00065/QUJDREVGRw"
                }
            ]
        }
        masked = capture_baseline.mask_stream_auth_tokens(document)
        self.assertTrue(masked["data"][0]["streamUrl"].endswith("<hmac-auth-token>"))

    def test_redact_body_keys(self) -> None:
        document = {"userName": "fixture", "password": "x", "nested": {"refreshToken": "y"}}
        redacted = capture_baseline.redact_body_keys(document)
        self.assertEqual(redacted["password"], "<redacted>")
        self.assertEqual(redacted["nested"]["refreshToken"], "<redacted>")
        self.assertEqual(redacted["userName"], "fixture")

    def test_unsubstitute_secrets(self) -> None:
        text = capture_baseline.unsubstitute_secrets(
            "http://h/a?jwt=tok123", {"accessToken": "tok123"}
        )
        self.assertNotIn("tok123", text)
        self.assertIn("<captured:accessToken>", text)


class StructureCheckTests(unittest.TestCase):
    def test_jwt_check_accepts_fixture_token(self) -> None:
        token = gen_fixture_tokens.make_jwt(
            legacy_oracle.FIXTURE_JWT_KEY,
            expired=False,
            user_api_key=legacy_oracle.fixture_uuid("user", legacy_oracle.FIXTURE_USER),
        )
        result = capture_baseline.check_jwt(token)
        self.assertTrue(result["valid"], result)
        self.assertEqual(result["issuer"], "MelodeeApi")
        self.assertEqual(result["audience"], "MelodeeClient")

    def test_jwt_check_rejects_garbage(self) -> None:
        self.assertFalse(capture_baseline.check_jwt("not-a-jwt")["valid"])

    def test_stream_auth_token_check(self) -> None:
        payload = (
            "f78448cc-82ec-5864-877c-6c6163b00065:2b8eab4b-235d-5892-99f8-c0f5f54e8ab4:127.0.0.1"
            "|1999999999|"
            + base64.b64encode(b"0" * 32).decode()
        )
        token = base64.b64encode(payload.encode()).decode().rstrip("=")
        result = capture_baseline.check_stream_auth_token(
            token,
            user_api_key="f78448cc-82ec-5864-877c-6c6163b00065",
            song_api_key="2b8eab4b-235d-5892-99f8-c0f5f54e8ab4",
        )
        self.assertTrue(result["valid"], result)
        self.assertEqual(result["binding"], "127.0.0.1")
        self.assertTrue(result["userMatchesUrl"])
        self.assertTrue(result["songMatchesUrl"])

    def test_stream_auth_token_rejects_wrong_shape(self) -> None:
        self.assertFalse(capture_baseline.check_stream_auth_token("!!!")["valid"])


class RouteInventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.inventory = json.loads(
            (ROOT / "compatibility" / "melodee-api-v1" / "contracts" / "route-inventory.json").read_text(
                encoding="utf-8"
            )
        )

    def route(self, method: str, path: str) -> dict:
        for entry in self.inventory["routes"]:
            if entry["method"] == method and entry["path"] == path:
                return entry
        raise AssertionError(f"route not in inventory: {method} {path}")

    def test_queue_is_excluded_from_build(self) -> None:
        entry = self.route("GET", "/api/v1/queue")
        self.assertEqual(entry["parityStatus"], "excluded-from-build")
        self.assertEqual(entry["implementation"], "excluded-from-build")
        self.assertFalse(entry["inOpenApi"])

    def test_stream_route_is_captured_and_client_used(self) -> None:
        entry = self.route("GET", "/song/stream/{}/{}/{}")
        self.assertEqual(entry["parityStatus"], "captured")
        clients = {usage["client"] for usage in entry["usedBy"]}
        self.assertEqual(clients, {"melodee-player", "meloamp"})
        self.assertGreaterEqual(len(entry["scenarios"]), 10)

    def test_openapi_is_not_sole_authority(self) -> None:
        not_in_openapi = [r for r in self.inventory["routes"] if not r["inOpenApi"]]
        self.assertTrue(not_in_openapi, "expected live routes missing from OpenAPI")
        self.assertIn(
            ("GET", "/api/v1/queue"),
            {(r["method"], r["path"]) for r in not_in_openapi},
        )

    def test_client_critical_routes_are_captured(self) -> None:
        critical = [
            ("POST", "/api/v1/auth/authenticate"),
            ("POST", "/api/v1/auth/refresh-token"),
            ("GET", "/api/v1/user/me"),
            ("GET", "/api/v1/artists"),
            ("GET", "/api/v1/albums/{}/songs"),
            ("GET", "/api/v1/playlists/{}/songs"),
            ("GET", "/api/v1/search/songs"),
            ("POST", "/api/v1/songs/starred/{}/{}"),
            ("POST", "/api/v1/scrobble"),
            ("GET", "/song/stream/{}/{}/{}"),
        ]
        for method, path in critical:
            entry = self.route(method, path)
            self.assertEqual(
                entry["parityStatus"],
                "captured",
                f"{method} {path} must be covered by a compatibility scenario",
            )


class GoldenFixtureTests(unittest.TestCase):
    def metadata(self) -> list[dict]:
        return [
            json.loads(path.read_text(encoding="utf-8"))
            for path in sorted(GOLDEN.glob("*/*.metadata.json"))
        ]

    def test_golden_directories_exist(self) -> None:
        for directory in ("native-auth", "native-catalog", "native-stream"):
            self.assertTrue((GOLDEN / directory / "provenance.json").exists())

    def test_no_credential_material_anywhere(self) -> None:
        import re

        jwt_shape = re.compile(rb"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")
        for path in GOLDEN.glob("*/*"):
            data = path.read_bytes()
            self.assertIsNone(jwt_shape.search(data), f"JWT-shaped value in {path}")
            self.assertNotIn(b"Mel0deeR0cks", data, f"fixture password in {path}")
            self.assertNotIn(b"REQUIRED: THIS MUST BE EDITED", data, f"placeholder baseUrl in {path}")
            if path.suffix in {".json", ".xml", ".txt"}:
                self.assertNotIn("127.0.0.1:5157", data.decode("utf-8"), f"raw origin in {path}")

    def test_fixture_public_ids_are_stable(self) -> None:
        login = (GOLDEN / "native-auth" / "auth-login-success.json").read_text(encoding="utf-8")
        self.assertIn("f78448cc-82ec-5864-877c-6c6163b00065", login)

    def test_tokens_are_masked_not_absent(self) -> None:
        login = json.loads((GOLDEN / "native-auth" / "auth-login-success.json").read_text())
        self.assertEqual(login["token"], "<volatile>")
        self.assertEqual(login["refreshToken"], "<volatile>")

    def test_structure_checks_recorded_for_token_scenarios(self) -> None:
        by_name = {m["name"]: m for m in self.metadata()}
        login_checks = by_name["auth-login-success"]["structureChecks"]
        self.assertTrue(login_checks["$.token"]["valid"])
        self.assertTrue(login_checks["$.refreshToken"]["valid"])
        stream_checks = by_name["stream/album-songs-for-stream-url"]["structureChecks"]
        check = stream_checks["$.data[0].streamUrl"]
        self.assertTrue(check["valid"])
        self.assertEqual(check["hmacShape"], "base64-sha256")
        self.assertEqual(check["expiresInDays"], 7)
        self.assertTrue(check["userMatchesUrl"])
        self.assertTrue(check["songMatchesUrl"])

    def test_stream_range_semantics_evidence(self) -> None:
        by_name = {m["name"]: m for m in self.metadata()}
        expectations = {
            "stream/head": (200, None),
            "stream/get-full": (200, None),
            "stream/get-range-initial": (206, "bytes 0-1023/220544"),
            "stream/get-range-final-suffix": (206, "bytes 218496-220543/220544"),
            "stream/get-range-malformed": (400, None),
            "stream/get-range-unsatisfiable": (400, None),
            "stream/get-token-for-wrong-song": (401, None),
        }
        for name, (status, content_range) in expectations.items():
            self.assertEqual(by_name[name]["response"]["status"], status, name)
            if content_range:
                self.assertEqual(
                    by_name[name]["response"]["headers"].get("content-range"), content_range, name
                )

    def test_auth_lifecycle_status_evidence(self) -> None:
        by_name = {m["name"]: m for m in self.metadata()}
        expectations = {
            "auth-login-success": 200,
            "auth-login-invalid-password": 401,
            "auth-login-unknown-user": 401,
            "auth-user-missing-token": 401,
            "auth-user-malformed-token": 401,
            "auth-user-expired-token": 401,
            "auth-refresh-rotation/rotate-first": 200,
            "auth-refresh-rotation/replay-rotated-token": 401,
            "auth-refresh-rotation/rotate-after-family-revoke": 401,
            "auth-refresh-rotation/logout": 200,
            "auth-refresh-rotation/access-after-logout": 200,
            "auth-refresh-rotation/refresh-after-logout": 401,
            "auth-refresh-expired-token": 401,
        }
        for name, status in expectations.items():
            self.assertEqual(by_name[name]["response"]["status"], status, name)

    def test_error_envelopes_preserved(self) -> None:
        invalid = json.loads(
            (GOLDEN / "native-auth" / "auth-login-invalid-password.json").read_text()
        )
        unknown = json.loads((GOLDEN / "native-auth" / "auth-login-unknown-user.json").read_text())
        # Unknown-user must not be distinguishable from wrong-password.
        self.assertEqual(invalid["code"], unknown["code"])
        self.assertEqual(invalid["message"], unknown["message"])
        replay = json.loads(
            (GOLDEN / "native-auth" / "auth-refresh-rotation-replay-rotated-token.json").read_text()
        )
        self.assertEqual(replay["code"], "refresh_token_replayed")


if __name__ == "__main__":
    unittest.main()
