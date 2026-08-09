"""Tests for Issue 6 OpenSubsonic compatibility capture and golden evidence."""
from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GOLDEN = ROOT / "compatibility" / "opensubsonic" / "golden"


class GoldenSecretScanTests(unittest.TestCase):
    def test_no_credential_material_anywhere(self) -> None:
        jwt_shape = re.compile(rb"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")
        fixture_token = b"d98eba44525df0051a8783705aa16c30"
        for path in GOLDEN.glob("*/*"):
            data = path.read_bytes()
            self.assertIsNone(jwt_shape.search(data), f"JWT-shaped value in {path}")
            self.assertNotIn(fixture_token, data, f"subsonic token in {path}")
            self.assertNotIn(fixture_token.upper(), data, f"uppercase subsonic token in {path}")
            self.assertNotIn(b"f1xture5a1t", data, f"subsonic salt in {path}")
            self.assertNotIn(b"phase0-opensubsonic-fixture-secret", data, f"opensubsonic secret in {path}")
            self.assertNotIn(b"REQUIRED: THIS MUST BE EDITED", data, f"placeholder baseUrl in {path}")

    def test_provenance_present(self) -> None:
        for directory in (
            "system", "auth", "browse", "lists", "media", "annotation",
            "playlists", "community", "negative", "conditional", "conditional-off",
        ):
            self.assertTrue((GOLDEN / directory / "provenance.json").exists(), directory)


def metadata_by_name(directory: str) -> dict[str, dict]:
    return {
        json.loads(path.read_text(encoding="utf-8"))["name"]: json.loads(path.read_text(encoding="utf-8"))
        for path in sorted((GOLDEN / directory).glob("*.metadata.json"))
    }


class AuthEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.meta = metadata_by_name("auth")

    def test_enforced_failures_are_error_40_http_200(self) -> None:
        for name in (
            "os-auth-invalid-username", "os-auth-invalid-token", "os-auth-malformed-token",
            "os-auth-token-without-salt", "os-auth-missing", "os-auth-apikey-only",
            "os-auth-jwt-only-unreachable",
        ):
            entry = self.meta[name]
            self.assertEqual(entry["response"]["status"], 200, name)
            body = json.loads((GOLDEN / "auth" / entry["response"]["bodyFile"]).read_text())
            self.assertEqual(body["subsonic-response"]["status"], "error", name)
            self.assertEqual(body["subsonic-response"]["error"]["code"], 40, name)

    def test_valid_forms_succeed(self) -> None:
        for name in (
            "os-auth-valid-token", "os-auth-valid-token-uppercase",
            "os-auth-valid-password-plain", "os-auth-valid-password-enc-hex",
            "os-auth-form-post", "os-auth-no-version-param", "os-auth-bypass-localhost",
        ):
            entry = self.meta[name]
            self.assertEqual(entry["response"]["status"], 200, name)
            body = json.loads((GOLDEN / "auth" / entry["response"]["bodyFile"]).read_text())
            self.assertEqual(body["subsonic-response"]["status"], "ok", name)

    def test_forged_unsigned_jwt_authenticates(self) -> None:
        """Security-critical: unsigned JWT + wrong password + valid username succeeds."""
        entry = self.meta["os-auth-jwt-forged-unsigned"]
        self.assertEqual(entry["response"]["status"], 200)
        body = json.loads((GOLDEN / "auth" / entry["response"]["bodyFile"]).read_text())
        self.assertEqual(body["subsonic-response"]["status"], "ok")

    def test_tokens_redacted_in_recorded_urls(self) -> None:
        for entry in self.meta.values():
            url = entry["request"]["url"]
            self.assertNotRegex(url, r"t=[0-9a-f]{32}")


class SystemAndFormatTests(unittest.TestCase):
    def test_extensions_advertised(self) -> None:
        body = json.loads((GOLDEN / "system" / "os-getopensubsonicextensions-json.json").read_text())
        names = {e["name"] for e in body["subsonic-response"]["openSubsonicExtensions"]}
        self.assertEqual(
            names,
            {"melodeeExtensions", "apiKeyAuthentication", "formPost", "songLyrics", "transcodeOffset"},
        )

    def test_xml_extensions_are_malformed(self) -> None:
        """The malformed XML extension shape is preserved as evidence."""
        xml = (GOLDEN / "system" / "os-getopensubsonicextensions-xml.xml").read_text()
        self.assertIn('name="melodeeExtensions" versions="1 "/>', xml)

    def test_jsonp_wrapper(self) -> None:
        text = (GOLDEN / "system" / "os-ping-jsonp.txt").read_text()
        self.assertTrue(text.startswith("phase0Callback({"))

    def test_xml_envelope_type_lowercase(self) -> None:
        xml = (GOLDEN / "system" / "os-ping-xml.xml").read_text()
        self.assertIn('type="melodee"', xml)

    def test_license_typo_preserved(self) -> None:
        body = json.loads((GOLDEN / "system" / "os-getlicense-json.json").read_text())
        self.assertIn("trailExpires", body["subsonic-response"]["license"])


class StreamEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.meta = metadata_by_name("media")

    def test_full_stream_200(self) -> None:
        entry = self.meta["os-stream-full"]
        self.assertEqual(entry["response"]["status"], 200)
        self.assertEqual(entry["response"]["headers"].get("content-type"), "audio/wav")
        self.assertEqual(entry["response"]["headers"].get("accept-ranges"), "bytes")
        self.assertEqual(entry["response"]["headers"].get("content-length"), "220544")

    def test_start_range_206(self) -> None:
        entry = self.meta["os-stream-range-start"]
        self.assertEqual(entry["response"]["status"], 206)
        # Double range-processing bug: even a "correct" start=0 range reports
        # the bounded stream length as the Content-Range total (1024), not the
        # true file size (220544). The body bytes are correct; the header lies.
        self.assertEqual(
            entry["response"]["headers"].get("content-range"), "bytes 0-1023/1024"
        )
        self.assertEqual(entry["response"]["bytes"], 1024)

    def test_double_range_processing_bug(self) -> None:
        """Nonzero-offset ranges 416 or mangle bodies at this baseline."""
        middle = self.meta["os-stream-range-middle"]
        self.assertEqual(middle["response"]["status"], 416)
        self.assertEqual(
            middle["response"]["headers"].get("content-range"), "bytes */1024"
        )
        mangled = self.meta["os-stream-range-middle-mangled"]
        self.assertEqual(mangled["response"]["status"], 206)
        self.assertEqual(
            mangled["response"]["headers"].get("content-range"), "bytes 100000-100000/100001"
        )
        self.assertEqual(mangled["response"]["bytes"], 1)

    def test_invalid_range_is_404_not_416(self) -> None:
        for name in ("os-stream-range-invalid", "os-stream-range-unsatisfiable", "os-stream-timeoffset"):
            self.assertEqual(self.meta[name]["response"]["status"], 404, name)

    def test_unknown_song_is_302(self) -> None:
        entry = self.meta["os-stream-unknown"]
        self.assertEqual(entry["response"]["status"], 302)
        self.assertNotRegex(
            entry["response"]["headers"].get("location", ""), r"t=[0-9a-f]{32}"
        )

    def test_download_has_content_disposition(self) -> None:
        entry = self.meta["os-download"]
        self.assertEqual(entry["response"]["status"], 200)
        self.assertIn(
            "attachment", entry["response"]["headers"].get("content-disposition", "")
        )


class NegativeAndConditionalTests(unittest.TestCase):
    def test_gone_routes_are_410(self) -> None:
        meta = metadata_by_name("negative")
        for name, entry in meta.items():
            if name.startswith("os-gone-"):
                self.assertEqual(entry["response"]["status"], 410, name)

    def test_not_implemented_routes_are_501(self) -> None:
        meta = metadata_by_name("negative")
        for name, entry in meta.items():
            if name.startswith("os-notimplemented-"):
                self.assertEqual(entry["response"]["status"], 501, name)

    def test_podcasts_disabled_is_400(self) -> None:
        meta = metadata_by_name("conditional-off")
        for name, entry in meta.items():
            self.assertEqual(entry["response"]["status"], 400, name)

    def test_podcast_auth_failure_is_403(self) -> None:
        meta = metadata_by_name("conditional")
        self.assertEqual(meta["os-getpodcasts-norole"]["response"]["status"], 403)


class RouteInventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.inventory = json.loads(
            (ROOT / "compatibility" / "opensubsonic" / "contracts" / "route-inventory.json").read_text(
                encoding="utf-8"
            )
        )

    def test_all_endpoints_classified(self) -> None:
        classifications = {e["classification"] for e in self.inventory["endpoints"]}
        self.assertTrue(classifications <= {"supported", "conditional", "gone", "not-implemented", "excluded-from-build"})
        summary = self.inventory["summary"]
        self.assertEqual(summary.get("gone"), 6)
        self.assertEqual(summary.get("not-implemented"), 4)
        self.assertEqual(self.inventory["endpointCount"], len(self.inventory["endpoints"]))

    def test_critical_endpoints_have_scenarios(self) -> None:
        critical = [
            "ping", "getLicense", "getOpenSubsonicExtensions", "getMusicFolders",
            "getIndexes", "getArtists", "getArtist", "getAlbum", "getSong", "getGenres",
            "getMusicDirectory", "getAlbumInfo", "getAlbumInfo2", "getArtistInfo",
            "getArtistInfo2", "getSimilarSongs", "getSimilarSongs2", "getTopSongs",
            "getAlbumList", "getAlbumList2", "getRandomSongs", "getSongsByGenre",
            "getNowPlaying", "getStarred", "getStarred2", "search2", "search3",
            "stream", "download", "getCoverArt", "getAvatar", "getLyrics",
            "getLyricsBySongId", "star", "unstar", "setRating", "scrobble",
            "getPlaylists", "getPlaylist", "createPlaylist", "updatePlaylist",
            "deletePlaylist", "getBookmarks", "createBookmark", "deleteBookmark",
            "getPlayQueue", "savePlayQueue", "getShares", "createShare", "updateShare",
            "deleteShare", "getInternetRadioStations", "createInternetRadioStation",
            "updateInternetRadioStation", "deleteInternetRadioStation", "startScan",
            "getScanStatus", "jukeboxControl", "getPodcasts", "getNewestPodcasts",
        ]
        by_endpoint = {e["endpoint"]: e for e in self.inventory["endpoints"]}
        for endpoint in critical:
            self.assertIn(endpoint, by_endpoint, endpoint)
            self.assertTrue(
                by_endpoint[endpoint]["hasGoldenScenario"],
                f"{endpoint} lacks golden scenario coverage",
            )

    def test_gone_and_501_have_scenarios(self) -> None:
        for entry in self.inventory["endpoints"]:
            if entry["classification"] in ("gone", "not-implemented"):
                self.assertTrue(
                    entry["hasGoldenScenario"],
                    f"{entry['endpoint']} ({entry['classification']}) lacks scenario",
                )
                self.assertIsNotNone(entry["expectedUnsupportedStatus"])


if __name__ == "__main__":
    unittest.main()
