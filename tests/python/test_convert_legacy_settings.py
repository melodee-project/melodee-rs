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


class ConvertLegacySettingsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module(
            "convert_legacy_settings", ROOT / "scripts" / "convert_legacy_settings.py"
        )
        cls.registry = cls.module.parse_registry(
            ROOT / cls.module.REGISTRY_RELATIVE_PATH
        )

    def test_registry_is_parsed_with_kinds_and_secrets(self) -> None:
        # The registry documents a 190-key legacy inventory; fail loudly if
        # the parser silently drops entries.
        self.assertGreaterEqual(len(self.registry), 190)
        self.assertEqual(self.registry["scrobbling.lastFm.Enabled"].kind, "Bool")
        self.assertFalse(self.registry["scrobbling.lastFm.Enabled"].secret)
        self.assertTrue(self.registry["scrobbling.lastFm.apiKey"].secret)
        bitrate = self.registry["conversion.bitrate"]
        self.assertEqual((bitrate.kind, bitrate.int_min, bitrate.int_max), ("Int", 1, None))
        vbr = self.registry["conversion.vbrLevel"]
        self.assertEqual((vbr.int_min, vbr.int_max), (0, 9))
        jukebox = self.registry["jukebox.backendType"]
        self.assertEqual(jukebox.kind, "Enum")
        self.assertEqual(jukebox.enum_options, ("", "mpv", "mpd"))
        volume = self.registry["mpd.initialVolume"]
        self.assertEqual((volume.kind, volume.float_min, volume.float_max), ("Float", 0.0, 1.0))
        secrets = [key for key, known in self.registry.items() if known.secret]
        self.assertEqual(len(secrets), 14)
        self.assertIn("email.smtpPassword", secrets)
        self.assertIn("encryption.privateKey", secrets)

    def test_value_conversion_covers_all_kinds_and_ranges(self) -> None:
        convert_value = self.module.convert_value
        self.assertEqual(convert_value(self.registry["conversion.enabled"], "True"), (True, None))
        self.assertEqual(convert_value(self.registry["conversion.enabled"], "no"), (False, None))
        value, error = convert_value(self.registry["conversion.enabled"], "maybe")
        self.assertIsNone(value)
        self.assertIn("boolean", error)
        self.assertEqual(convert_value(self.registry["conversion.vbrLevel"], "7"), (7, None))
        _, out_of_range = convert_value(self.registry["conversion.vbrLevel"], "12")
        self.assertEqual(out_of_range, "value above the allowed maximum")
        self.assertEqual(convert_value(self.registry["jukebox.backendType"], "mpd"), ("mpd", None))
        _, bad_enum = convert_value(self.registry["jukebox.backendType"], "m3u")
        self.assertIn("allowed options", bad_enum)
        self.assertEqual(convert_value(self.registry["mpd.initialVolume"], "0.5"), (0.5, None))
        _, bad_float = convert_value(self.registry["mpd.initialVolume"], "4.2")
        self.assertIn("above the allowed maximum", bad_float)
        # Text values are kept verbatim (legacy templates and cron strings).
        self.assertEqual(
            convert_value(self.registry["jobs.chartUpdate.cronExpression"], "0 0 12 1 * ?"),
            ("0 0 12 1 * ?", None),
        )

    def test_document_building_redacts_and_never_imports_quirk_keys(self) -> None:
        rows = [
            {"Key": "conversion.enabled", "Value": "True"},
            {"Key": "email.smtpPassword", "Value": "s3cret"},
            {"Key": "encryption.privateKey", "Value": "legacy-key"},
            {"Key": "unknown.legacyKey", "Value": "x"},
            {"Key": "empty.key", "Value": ""},
        ]
        document, report = self.module.build_document(rows, self.registry, include_secrets=False)
        self.assertEqual(document["format"], self.module.CONFIG_EXPORT_FORMAT)
        self.assertEqual(document["warning"], self.module.CONFIG_EXPORT_WARNING)
        self.assertEqual(document["settings"], {"conversion.enabled": True})
        self.assertNotIn("s3cret", json.dumps(document))
        self.assertEqual(report.converted, ["conversion.enabled"])
        self.assertEqual(report.secrets_omitted, ["email.smtpPassword"])
        self.assertEqual(report.never_imported, ["encryption.privateKey"])
        self.assertEqual(report.unknown_keys, ["unknown.legacyKey"])
        self.assertEqual(report.empty_values, ["empty.key"])

    def test_include_secrets_carries_values_for_reentry(self) -> None:
        rows = [
            {"Key": "email.smtpPassword", "Value": "s3cret"},
            {"Key": "encryption.privateKey", "Value": "legacy-key"},
        ]
        document, report = self.module.build_document(rows, self.registry, include_secrets=True)
        self.assertEqual(document["settings"], {"email.smtpPassword": "s3cret"})
        self.assertEqual(report.secrets_included, ["email.smtpPassword"])
        # The quirk keys are excluded even when secrets are requested.
        self.assertEqual(report.never_imported, ["encryption.privateKey"])
        self.assertNotIn("legacy-key", json.dumps(document))

    def test_conversion_errors_are_reported_without_echoing_values(self) -> None:
        rows = [
            {"Key": "conversion.vbrLevel", "Value": "banana"},
            {"Key": "email.smtpPort", "Value": "99999"},
        ]
        _, report = self.module.build_document(rows, self.registry, include_secrets=False)
        self.assertEqual(len(report.errors), 2)
        for error in report.errors:
            self.assertNotIn("banana", error)
            self.assertNotIn("99999", error)

    def test_missing_csv_columns_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "bad.csv"
            bad.write_text("Id,Name\n1,x\n", encoding="utf-8")
            with self.assertRaises(SystemExit):
                self.module.read_csv_rows(bad)

    def test_main_is_safe_and_helpful(self) -> None:
        with self.assertRaises(SystemExit) as caught:
            self.module.main(["--help"])
        self.assertEqual(caught.exception.code, 0)
        with self.assertRaises(SystemExit) as caught:
            self.module.main([])
        self.assertNotEqual(caught.exception.code, 0)

    def test_end_to_end_cli_conversion_writes_expected_document(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            csv_path = Path(tmp) / "settings.csv"
            csv_path.write_text(
                "Key,Value\n"
                "conversion.enabled,True\n"
                "conversion.vbrLevel,6\n"
                "email.smtpHost,smtp.example.net\n"
                "scrobbling.lastFm.apiKey,abc123\n"
                "encryption.privateKey,never-this\n"
                "totally.unknown,true\n",
                encoding="utf-8",
            )
            output = Path(tmp) / "import.json"
            exit_code = self.module.main(["--csv", str(csv_path), "--output", str(output)])
            self.assertEqual(exit_code, 0)
            document = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(document["format"], "melodee-config-export/v1")
            self.assertEqual(
                document["settings"],
                {
                    "conversion.enabled": True,
                    "conversion.vbrLevel": 6,
                    "email.smtpHost": "smtp.example.net",
                },
            )
            rendered = output.read_text(encoding="utf-8")
            self.assertNotIn("abc123", rendered)
            self.assertNotIn("never-this", rendered)
            # A second run without --force refuses to overwrite.
            with self.assertRaises(SystemExit):
                self.module.main(["--csv", str(csv_path), "--output", str(output)])


if __name__ == "__main__":
    unittest.main()
