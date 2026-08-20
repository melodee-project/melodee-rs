"""Tests for Issue 5 native API v1 capture tooling and golden fixtures.

Covers: oracle fixture determinism, capture-time sanitization (no credential
material may persist), token structure validators, the unified route
inventory, and the committed golden evidence itself.
"""
from __future__ import annotations

import base64
import copy
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
build_phase7_route_matrix = load_module(
    "build_phase7_route_matrix", ROOT / "scripts" / "build_phase7_route_matrix.py"
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

    def test_native_share_fixture_can_be_isolated_from_other_surfaces(self) -> None:
        catalog = json.loads(
            (ROOT / "compatibility" / "fixtures" / "catalog-v1.json").read_text(encoding="utf-8")
        )
        media = {
            song["slug"]: (100, "0" * 64)
            for artist in catalog["artists"]
            for album in artist["albums"]
            for song in album["songs"]
        }
        native_sql = legacy_oracle.seed_sql(catalog, media)
        surface_sql = legacy_oracle.seed_sql(catalog, media, include_native_share=False)

        self.assertIn("phase7-public-song", native_sql)
        self.assertNotIn("phase7-public-song", surface_sql)

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

    def test_integer_path_parameters_normalize_in_any_segment(self) -> None:
        self.assertEqual(
            build_native_inventory.normalize_path(
                "/api/v1/party-sessions/00000000-0000-0000-0000-000000000001/participants/1/ban"
            ),
            "/api/v1/party-sessions/{}/participants/{}/ban",
        )

        manifest = json.loads(
            (ROOT / "compatibility/melodee-api-v1/contracts/route-manifest.json").read_text()
        )
        fixed_numeric_segments = [
            route["path"]
            for route in manifest["routes"]
            if any(segment.isdigit() for segment in route["path"].split("/"))
        ]
        self.assertEqual(
            fixed_numeric_segments,
            [],
            "normalizing integer components is safe only while the frozen manifest has no fixed numeric segments",
        )

    def test_inventory_path_maps_concrete_opaque_negative_capture(self) -> None:
        mapping = build_native_inventory.scenario_route_map()
        self.assertIn(
            "auth-password-reset-validate-invalid",
            mapping[("GET", "/api/v1/auth/password-reset/validate/{}")],
        )

    def test_matrix_rejects_fabricated_rust_test_reference(self) -> None:
        inventory = copy.deepcopy(self.inventory)
        evidence = json.loads(
            (ROOT / "compatibility/melodee-api-v1/contracts/phase7-route-evidence.json").read_text()
        )
        evidence["routes"][0]["rustTests"] = ["does/not/exist.rs::fabricated_test"]
        with self.assertRaisesRegex(build_phase7_route_matrix.MatrixError, "file is missing"):
            build_phase7_route_matrix.build_matrix(inventory, evidence)

    def test_matrix_accepts_only_runnable_rust_test_symbols(self) -> None:
        import tempfile

        fake_sources = {
            "block_comment": "/* #[test]\nfn proof() {} */\n",
            "line_comment": "// #[test]\n// fn proof() {}\n",
            "ordinary_string": 'const TEXT: &str = "#[test]\nfn proof() {}";\n',
            "raw_string": 'const TEXT: &str = r#"#[tokio::test]\nasync fn proof() {}"#;\n',
            "ignored": "#[ignore = \"not evidence\"]\n#[tokio::test]\nasync fn proof() {}\n",
            "ordinary_function": "fn proof() {}\n",
        }
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source_path = root / "tests" / "proof.rs"
            source_path.parent.mkdir()
            reference = "tests/proof.rs::proof"
            for label, source in fake_sources.items():
                with self.subTest(label=label):
                    source_path.write_text(source, encoding="utf-8")
                    with self.assertRaisesRegex(
                        build_phase7_route_matrix.MatrixError,
                        "named Rust test is missing",
                    ):
                        build_phase7_route_matrix.validate_rust_tests(
                            [reference], ("GET", "/proof"), root
                        )

            source_path.write_text(
                "#[tokio::test]\nasync fn proof() {}\n", encoding="utf-8"
            )
            build_phase7_route_matrix.validate_rust_tests(
                [reference], ("GET", "/proof"), root
            )

    def test_promoted_golden_hash_and_provenance_are_verified(self) -> None:
        import tempfile

        source = GOLDEN / "native-community"
        metadata_source = source / "community-lifecycle-request-list.metadata.json"
        metadata = json.loads(metadata_source.read_text())
        body_name = metadata["response"]["bodyFile"]
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "native-community"
            directory.mkdir()
            (directory / "provenance.json").write_text(
                (source / "provenance.json").read_text(), encoding="utf-8"
            )
            (directory / metadata_source.name).write_text(
                json.dumps(metadata), encoding="utf-8"
            )
            (directory / body_name).write_bytes(b"corrupt")
            original = build_native_inventory.GOLDEN_DIR
            build_native_inventory.GOLDEN_DIR = Path(tmp)
            try:
                with self.assertRaisesRegex(ValueError, "hash mismatch"):
                    build_native_inventory.promoted_golden_names()
            finally:
                build_native_inventory.GOLDEN_DIR = original

    def test_promoted_golden_method_and_path_must_match_scenario(self) -> None:
        import tempfile

        source = GOLDEN / "native-community"
        metadata_source = source / "community-lifecycle-request-list.metadata.json"
        metadata = json.loads(metadata_source.read_text())
        metadata["request"]["method"] = "DELETE"
        body_name = metadata["response"]["bodyFile"]
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "native-community"
            directory.mkdir()
            (directory / "provenance.json").write_text(
                (source / "provenance.json").read_text(), encoding="utf-8"
            )
            (directory / metadata_source.name).write_text(json.dumps(metadata), encoding="utf-8")
            (directory / body_name).write_bytes((source / body_name).read_bytes())
            original = build_native_inventory.GOLDEN_DIR
            build_native_inventory.GOLDEN_DIR = Path(tmp)
            try:
                with self.assertRaisesRegex(ValueError, "method/path"):
                    build_native_inventory.promoted_golden_names()
            finally:
                build_native_inventory.GOLDEN_DIR = original

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

    def test_capture_status_requires_promoted_golden_evidence(self) -> None:
        promoted = build_native_inventory.promoted_golden_names()
        self.assertGreater(len(promoted), 0)
        for route in self.inventory["routes"]:
            self.assertEqual(
                route["scenarios"],
                [name for name in route["declaredScenarios"] if name in promoted],
            )
            if route["parityStatus"] == "captured":
                self.assertGreater(len(route["scenarios"]), 0)


class Phase7RouteMatrixTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.inventory = json.loads(
            (ROOT / "compatibility/melodee-api-v1/contracts/route-inventory.json").read_text()
        )
        cls.evidence = json.loads(
            (ROOT / "compatibility/melodee-api-v1/contracts/phase7-route-evidence.json").read_text()
        )
        cls.matrix = json.loads(
            (ROOT / "design/agents/PHASE-7-ROUTE-MATRIX.json").read_text()
        )

    def phase7_route(self, method: str, path: str) -> dict:
        return next(
            route
            for route in self.matrix["routes"]
            if route["method"] == method and route["path"] == path
        )

    def test_matrix_is_generated_from_inventory_and_rust_evidence(self) -> None:
        generated = build_phase7_route_matrix.build_matrix(self.inventory, self.evidence)
        self.assertEqual(self.matrix, generated)
        self.assertEqual(self.matrix["routeCount"], 202)
        self.assertEqual(
            self.matrix["pendingRouteCount"]
            + self.matrix["completedRouteCount"]
            + self.matrix["deferredRouteCount"],
            202,
        )

    def test_legacy_capture_is_not_a_rust_implementation_claim(self) -> None:
        matrix_without_rust_evidence = build_phase7_route_matrix.build_matrix(
            self.inventory,
            {**self.evidence, "routes": []},
        )
        route = next(
            route
            for route in matrix_without_rust_evidence["routes"]
            if route["method"] == "GET" and route["path"] == "/api/v1/artists"
        )
        self.assertEqual(route["legacyContract"]["captureStatus"], "captured")
        self.assertEqual(route["phase7"]["status"], "implementation-pending")

    def test_excluded_party_and_queue_routes_have_legacy_evidence_but_need_rust_proof(self) -> None:
        queue = self.phase7_route("GET", "/api/v1/queue")
        party = self.phase7_route("POST", "/api/v1/party-sessions")
        self.assertGreater(len(queue["legacyContract"]["scenarios"]), 0)
        self.assertGreater(len(party["legacyContract"]["scenarios"]), 0)
        for route in (queue, party):
            self.assertIn(
                route["phase7"]["status"],
                {"negative-compatibility-pending", "preserved-intentional-legacy-absence"},
            )

    def test_compiled_overlap_domains_remain_phase7_obligations(self) -> None:
        for method, path in (
            ("GET", "/api/v1/analytics/listening"),
            ("GET", "/api/v1/requests"),
            ("GET", "/api/v1/shares"),
            ("GET", "/api/v1/podcasts/channels"),
        ):
            route = self.phase7_route(method, path)
            self.assertEqual(route["legacyContract"]["implementation"], "implemented")
            self.assertIn(
                route["phase7"]["status"],
                {
                    "implementation-pending",
                    "implemented-and-differentially-tested",
                    "approved-compatibility-normalization",
                },
            )

    def test_native_client_cross_surface_images_require_rust_evidence(self) -> None:
        rows = self.matrix["crossSurfaceDependencies"]
        self.assertEqual(len(rows), 5)
        self.assertTrue(
            all(
                row["phase7"]["status"] == "implemented-and-differentially-tested"
                and row["phase7"]["rustTests"]
                for row in rows
            )
        )
        without_evidence = build_phase7_route_matrix.build_matrix(
            self.inventory,
            {**self.evidence, "crossSurfaceDependencies": []},
        )
        self.assertTrue(
            all(
                row["phase7"]["status"] == "implementation-pending"
                for row in without_evidence["crossSurfaceDependencies"]
            )
        )

    def test_evidence_cannot_claim_differential_parity_without_tests(self) -> None:
        evidence = {**self.evidence, "routes": [{
            "method": "GET",
            "path": "/api/v1/artists",
            "status": "implemented-and-differentially-tested",
            "authority": "PRD-API-M1-001",
            "requirements": ["PRD-API-M1-001"],
            "legacyEvidence": ["catalog/artists-list"],
            "rustTests": [],
        }]}
        with self.assertRaisesRegex(build_phase7_route_matrix.MatrixError, "rustTests"):
            build_phase7_route_matrix.build_matrix(self.inventory, evidence)

    def test_preserved_absence_requires_accepted_adr_authority(self) -> None:
        evidence = {**self.evidence, "routes": [{
            "method": "GET",
            "path": "/api/v1/queue",
            "status": "preserved-intentional-legacy-absence",
            "authority": "PRD-USER-004",
            "requirements": ["PRD-USER-004"],
            "legacyEvidence": ["catalog-queue/queue-get-empty"],
            "rustTests": ["native_queue_absence_is_preserved"],
        }]}
        with self.assertRaisesRegex(build_phase7_route_matrix.MatrixError, "accepted ADR"):
            build_phase7_route_matrix.build_matrix(self.inventory, evidence)


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
