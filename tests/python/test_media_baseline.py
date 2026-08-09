"""Tests for the Issue 8 media-processing baseline artifacts."""
from __future__ import annotations

import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MEDIA = ROOT / "compatibility" / "media"


class BaselineStructureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.baseline = json.loads((MEDIA / "baseline.json").read_text(encoding="utf-8"))
        cls.cases = {
            p.stem: json.loads(p.read_text(encoding="utf-8"))
            for p in sorted((MEDIA / "cases").glob("*.json"))
        }

    def test_baseline_identifies_oracle(self) -> None:
        self.assertEqual(
            self.baseline["legacyBaselineCommit"],
            "4028b58dbd3302ed431e2c5b32d9dde5634db218",
        )

    def test_all_cases_have_input_and_expected(self) -> None:
        self.assertGreaterEqual(len(self.cases), 30)
        for slug, case in self.cases.items():
            self.assertTrue(case["input"]["files"], slug)
            self.assertTrue(case["expected"], slug)

    def test_seeded_conversion_settings(self) -> None:
        settings = self.baseline["environment"]["seededSettings"]
        self.assertEqual(settings["conversion.enabled"], "true")
        self.assertEqual(settings["conversion.bitrate"], "384")
        self.assertEqual(settings["conversion.samplingRate"], "48000")
        self.assertEqual(settings["transcoding.default"], "raw")

    def test_no_credentials_in_cases(self) -> None:
        for path in MEDIA.glob("**/*.json"):
            data = path.read_bytes()
            self.assertNotIn(b"Mel0deeR0cks", data)
            self.assertNotIn(b"phase0-fixture-only-security", data)
            self.assertNotIn(b"phase0-opensubsonic-fixture-secret", data)


class BehaviorEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.cases = {
            p.stem: json.loads(p.read_text(encoding="utf-8"))
            for p in sorted((MEDIA / "cases").glob("*.json"))
        }

    def album(self, slug: str) -> dict:
        albums = self.cases[slug]["pipeline"]["albums"]
        self.assertTrue(albums, slug)
        return albums[0]

    def test_flac_conversion_outcome(self) -> None:
        album = self.album("clean-flac")
        names = [s["fileName"] for s in album["songs"]]
        self.assertEqual(names, ["0001 First Light.mp3", "0002 Second Wave.mp3", "0003 Third Echo.mp3"])
        self.assertEqual(album["images"][0]["name"], "i-01-Front.jpg")
        leftovers = [t["path"] for t in self.cases["clean-flac"]["pipeline"]["inboundLeftovers"]]
        self.assertTrue(any(p.endswith(".flac._converted") for p in leftovers))

    def test_metadata_container_gate(self) -> None:
        """WAV cases produce zero albums (F1)."""
        for slug in ("clean-wav", "multichannel", "not-mp3-named-mp3"):
            self.assertEqual(self.cases[slug]["pipeline"]["albums"], [], slug)
            extraction = self.cases[slug]["extraction"]["extractions"][0]
            self.assertIsNone(extraction["tags"].get("Title"), slug)

    def test_same_size_deletion_bug(self) -> None:
        """F2: one of the two differing songs at identical size was deleted
        (which one is race-dependent); melodee.json still references both."""
        album = self.album("same-size-files")
        staged = {t["path"].split("/")[-1] for t in self.cases["same-size-files"]["pipeline"]["stagingTree"]}
        staged_songs = {name for name in staged if name.endswith(".mp3")}
        self.assertEqual(len(staged_songs), 1, f"exactly one song file must survive: {staged_songs}")
        referenced = {s["fileName"] for s in album["songs"]}
        self.assertEqual(len(referenced), 2, "melodee.json still references both songs")

    def test_artist_nullification(self) -> None:
        extraction = self.cases["clean-mp3"]["extraction"]["extractions"][0]
        self.assertIsNone(extraction["tags"].get("Artist"))
        self.assertEqual(extraction["tags"].get("AlbumArtist"), "Corpus Artist")

    def test_oversized_artwork_resized(self) -> None:
        album = self.album("oversized-artwork")
        image = album["images"][0]
        self.assertEqual((image["width"], image["height"]), (1600, 1600))

    def test_invalid_artwork_deleted(self) -> None:
        album = self.album("invalid-artwork")
        self.assertEqual(album["images"], [])
        self.assertIn("HasNoImages", album["statusReasons"])

    def test_unicode_filename_stripping(self) -> None:
        album = self.album("unicode-tags")
        names = [s["fileName"] for s in album["songs"]]
        self.assertIn("0001 Neon.mp3", names)
        self.assertTrue(all("東京" not in n for n in names))
        self.assertIn("東京", self.cases["unicode-tags"]["pipeline"]["albums"][0]["displaySummary"] or "")

    def test_multidisc_sortorder_convention(self) -> None:
        album = self.album("multi-disc")
        disc2_songs = [s for s in album["songs"] if s["mediaNumber"] == 2]
        self.assertEqual(len(disc2_songs), 2)
        self.assertEqual(
            sorted(s["sortOrder"] for s in disc2_songs), [10001, 10002],
            "SortOrder = SongNumber + disc*10000 - 10000",
        )

    def test_live_classification(self) -> None:
        album = self.album("title-live")
        self.assertEqual(album["albumType"], "Other")
        self.assertIn("IsNotStudioTypeAlbum", album["statusReasons"])

    def test_corrupt_audio_skips_song(self) -> None:
        album = self.album("corrupt-audio")
        self.assertEqual(album["songCount"], 2)

    def test_validator_results_present(self) -> None:
        with_validation = 0
        for slug, case in self.cases.items():
            for validation in case["pipeline"].get("validations", []):
                if validation.get("result", {}).get("data"):
                    with_validation += 1
        self.assertGreaterEqual(with_validation, 28)


if __name__ == "__main__":
    unittest.main()
