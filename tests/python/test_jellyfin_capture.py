"""Tests for Issue 7 Jellyfin compatibility capture and golden evidence."""
from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GOLDEN = ROOT / "compatibility" / "jellyfin" / "golden"


class GoldenSecretScanTests(unittest.TestCase):
    def test_no_credential_material_anywhere(self) -> None:
        jwt_shape = re.compile(rb"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")
        for path in GOLDEN.glob("*/*"):
            data = path.read_bytes()
            self.assertIsNone(jwt_shape.search(data), f"JWT-shaped value in {path}")
            self.assertNotIn(b"REQUIRED: THIS MUST BE EDITED", data)

    def test_access_tokens_masked(self) -> None:
        meta = json.loads((GOLDEN / "auth" / "jf-auth-login.metadata.json").read_text())
        checks = meta.get("structureChecks") or {}
        self.assertTrue(checks.get("$.AccessToken", {}).get("valid"))
        body = json.loads((GOLDEN / "auth" / "jf-auth-login.json").read_text())
        self.assertEqual(body["AccessToken"], "<volatile>")

    def test_provenance_present(self) -> None:
        for directory in ("discovery", "auth", "browse", "playback", "userstate", "playlists", "images", "extra"):
            self.assertTrue((GOLDEN / directory / "provenance.json").exists(), directory)


def metadata_by_name(directory: str) -> dict[str, dict]:
    result = {}
    for path in sorted((GOLDEN / directory).glob("*.metadata.json")):
        entry = json.loads(path.read_text(encoding="utf-8"))
        result[entry["name"]] = entry
    return result


class RoutingEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.meta = metadata_by_name("discovery")

    def test_root_fallthrough_without_markers(self) -> None:
        entry = self.meta["jf-root-no-markers"]
        self.assertEqual(entry["response"]["status"], 200)
        body_file = entry["response"]["bodyFile"]
        self.assertTrue(body_file.endswith(".xml"), "Blazor HTML page expected")

    def test_root_head_with_marker_is_rewritten(self) -> None:
        self.assertEqual(self.meta["jf-root-head-mediabrowser"]["response"]["status"], 200)

    def test_system_info_public_anonymous(self) -> None:
        entry = self.meta["jf-system-info-public"]
        self.assertEqual(entry["response"]["status"], 200)
        body = json.loads((GOLDEN / "discovery" / entry["response"]["bodyFile"]).read_text())
        self.assertEqual(body["ProductName"], "Melodee")
        self.assertTrue(body["StartupWizardCompleted"])

    def test_unsupported_scope_is_404(self) -> None:
        for name in ("jf-unsupported-videos", "jf-unsupported-livetv", "jf-unsupported-shows"):
            self.assertEqual(self.meta[name]["response"]["status"], 404, name)


class AuthEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.meta = metadata_by_name("auth")

    def test_all_transports_accepted(self) -> None:
        for name in (
            "jf-auth/me-mediabrowser-header", "jf-auth/me-x-mediabrowser-token",
            "jf-auth/me-x-emby-token", "jf-auth/me-x-emby-authorization",
            "jf-auth/me-api-key-query",
        ):
            self.assertEqual(self.meta[name]["response"]["status"], 200, name)

    def test_password_field_not_read(self) -> None:
        self.assertEqual(self.meta["jf-auth-negative/password-field-not-read"]["response"]["status"], 400)

    def test_logout_kills_token(self) -> None:
        self.assertEqual(self.meta["jf-auth/logout"]["response"]["status"], 204)
        self.assertEqual(self.meta["jf-auth/me-after-logout"]["response"]["status"], 401)

    def test_negative_logins(self) -> None:
        self.assertEqual(self.meta["jf-auth-negative/wrong-password"]["response"]["status"], 401)
        self.assertEqual(self.meta["jf-auth-negative/unknown-user"]["response"]["status"], 401)


class PlaybackEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.meta = metadata_by_name("playback")

    def test_correct_range_handling(self) -> None:
        middle = self.meta["jf-playback/stream-range-middle"]
        self.assertEqual(middle["response"]["status"], 206)
        self.assertEqual(
            middle["response"]["headers"].get("content-range"), "bytes 100000-101023/220544"
        )
        self.assertEqual(middle["response"]["bytes"], 1024)

    def test_invalid_range_is_416(self) -> None:
        self.assertEqual(self.meta["jf-playback/stream-range-invalid"]["response"]["status"], 416)
        self.assertEqual(self.meta["jf-playback/stream-range-unsatisfiable"]["response"]["status"], 416)

    def test_transcode_rejection(self) -> None:
        entry = self.meta["jf-playback/stream-extension-no-static"]
        self.assertEqual(entry["response"]["status"], 400)
        body = json.loads((GOLDEN / "playback" / entry["response"]["bodyFile"]).read_text())
        self.assertIn("Transcoding is not supported", body.get("Detail", body.get("detail", "")))

    def test_static_ignores_container(self) -> None:
        entry = self.meta["jf-playback/stream-extension-static"]
        self.assertEqual(entry["response"]["status"], 200)
        self.assertEqual(entry["response"]["headers"].get("content-type"), "audio/wav")

    def test_playbackinfo_no_transcoding(self) -> None:
        entry = self.meta["jf-playback/playbackinfo"]
        body = json.loads((GOLDEN / "playback" / entry["response"]["bodyFile"]).read_text())
        self.assertFalse(body["MediaSources"][0]["SupportsTranscoding"])


class PlaylistEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.meta = metadata_by_name("playlists")

    def test_move_and_remove_roundtrip(self) -> None:
        after_move = json.loads((GOLDEN / "playlists" / "jf-playlists-items-after-move.json").read_text())
        self.assertEqual(after_move["Items"][0]["Name"], "Coda")
        after_remove = json.loads((GOLDEN / "playlists" / "jf-playlists-items-after-remove.json").read_text())
        names = [item["Name"] for item in after_remove["Items"]]
        self.assertNotIn("Solar Wind", names)

    def test_add_boundary(self) -> None:
        self.assertEqual(self.meta["jf-playlists/add-first-empty"]["response"]["status"], 204)
        self.assertEqual(self.meta["jf-playlists/add-second-nonempty"]["response"]["status"], 500)


class RouteInventoryTests(unittest.TestCase):
    def test_all_routes_have_scenarios(self) -> None:
        inventory = json.loads(
            (ROOT / "compatibility" / "jellyfin" / "contracts" / "route-inventory.json").read_text()
        )
        uncovered = [r for r in inventory["routes"] if not r["hasGoldenScenario"]]
        self.assertEqual(uncovered, [], f"routes without scenarios: {uncovered}")
        self.assertEqual(inventory["routeCount"], inventory["coveredCount"])


if __name__ == "__main__":
    unittest.main()
