"""The language-selector completeness indicator must match shipped catalogs."""

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
from check_locales import catalog_coverage, validate_coverage


class LocaleCoverageTests(unittest.TestCase):
    def test_partial_catalogs_are_allowed_and_counted_without_english_fallback(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "en-US.json").write_text(json.dumps({"one": "One", "two": "Two"}))
            (root / "de-DE.json").write_text(json.dumps({"one": "Eins"}))
            self.assertEqual(catalog_coverage(root)["de-DE"], {"present": 1, "total": 2})

    def test_unknown_or_empty_translations_fail(self):
        for translation in ({"unknown": "value"}, {"one": " "}, {"one": 1}):
            with self.subTest(translation=translation), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "en-US.json").write_text('{"one":"One"}')
                (root / "de-DE.json").write_text(json.dumps(translation))
                with self.assertRaises(ValueError):
                    catalog_coverage(root)

    def test_catalog_growth_invalidates_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            locales = root / "locales"
            locales.mkdir()
            source = locales / "en-US.json"
            source.write_text('{"one":"One"}')
            manifest = root / "coverage.json"
            validate_coverage(locales, manifest, write=True)
            validate_coverage(locales, manifest)
            source.write_text('{"one":"One", "two":"Two"}')
            with self.assertRaisesRegex(ValueError, "stale"):
                validate_coverage(locales, manifest)


if __name__ == "__main__":
    unittest.main()
