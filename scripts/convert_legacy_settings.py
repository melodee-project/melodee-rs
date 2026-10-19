#!/usr/bin/env python3
"""Convert a legacy C# Melodee settings-table export into a Rust import document.

The Rust server keeps the legacy ``server_settings`` key inventory verbatim
(``KNOWN_SETTINGS`` in ``crates/melodee-persistence/src/settings.rs``), so a
C# ``_Settings`` table export maps key-for-key onto the
``melodee-config-export/v1`` import document consumed by
``melodee-cli config import``.

Safety contract (PRD-AUTH-007):
- Secret values are omitted from the document unless ``--include-secrets`` is
  given; every secret key is always reported by name so the operator can
  re-enter it deliberately. Secret values are never printed.
- ``encryption.privateKey`` and ``jellyfin.token.pepper`` are NEVER emitted:
  the Rust registry deliberately does not carry their legacy defaults and
  both must be regenerated, never imported (inventory quirk 4).
- Values are typed against the live registry (bool/int/float/enum/text) and
  range-checked; value text is never echoed in errors.

The registry is parsed from the Rust source at run time so the converter
cannot drift from the compiled behavior.

This script is read-only with respect to its input; it writes only the
output document path (refusing to overwrite without ``--force``).
"""

from __future__ import annotations

import argparse
import csv
import datetime
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import IO, Optional

CONFIG_EXPORT_FORMAT = "melodee-config-export/v1"
CONFIG_EXPORT_WARNING = (
    "This file contains server configuration. Secret values are REDACTED and "
    "must be re-entered after import; redacted markers are never accepted as "
    "values. Handle this file as sensitive: non-secret values can still "
    "reveal infrastructure details."
)
MAX_SETTING_VALUE_BYTES = 64 * 1024
MAX_IMPORT_KEYS = 1000

# Keys whose stored value must be regenerated on the Rust side, never
# imported from any legacy source (settings.rs module docs, quirk 4).
NEVER_IMPORT_KEYS = frozenset({"encryption.privateKey", "jellyfin.token.pepper"})

REGISTRY_RELATIVE_PATH = Path("crates/melodee-persistence/src/settings.rs")
_KIND_LINE = re.compile(
    r'KnownSetting\s*\{\s*key:\s*"(?P<key>[^"]+)",\s*'
    r"kind:\s*(?P<kind>.*?),\s*secret:\s*(?P<secret>true|false),"
)
_INT_RANGE = re.compile(r"Int\s*\{\s*min:\s*(?P<min>-?\d+),\s*max:\s*(?P<max>Some\(-?\d+\)|None)\s*\}")
_FLOAT_RANGE = re.compile(
    r"Float\s*\{\s*min:\s*(?P<min>-?\d+\.\d+),\s*max:\s*(?P<max>-?\d+\.\d+)\s*\}"
)
_ENUM_OPTIONS = re.compile(r'Enum\(&\[(?P<options>[^\]]*)\]')


@dataclass
class KnownSetting:
    key: str
    kind: str
    secret: bool
    int_min: Optional[int] = None
    int_max: Optional[int] = None
    float_min: Optional[float] = None
    float_max: Optional[float] = None
    enum_options: tuple[str, ...] = field(default=())


@dataclass
class ConversionReport:
    converted: list[str] = field(default_factory=list)
    secrets_included: list[str] = field(default_factory=list)
    secrets_omitted: list[str] = field(default_factory=list)
    never_imported: list[str] = field(default_factory=list)
    unknown_keys: list[str] = field(default_factory=list)
    empty_values: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)


def parse_registry(settings_rs: Path) -> dict[str, KnownSetting]:
    """Parses ``KNOWN_SETTINGS`` entries out of the Rust source (fail-closed)."""
    if not settings_rs.is_file():
        raise SystemExit(f"error: registry source not found: {settings_rs}")
    registry: dict[str, KnownSetting] = {}
    for match in _KIND_LINE.finditer(settings_rs.read_text(encoding="utf-8")):
        key = match.group("key")
        kind_raw = match.group("kind").strip()
        # Examples: "SettingKind::Bool", "SettingKind::Int { min: 1, max: Some(9) }",
        # "SettingKind::Enum(&[\"\", \"mpv\", \"mpd\"])". The leading "(" split
        # first keeps Enum literals with embedded spaces intact.
        kind = kind_raw.removeprefix("SettingKind::").split("(", 1)[0].split(" ", 1)[0].strip()
        known = KnownSetting(key=key, kind=kind, secret=match.group("secret") == "true")
        if int_match := _INT_RANGE.search(kind_raw):
            known.int_min = int(int_match.group("min"))
            max_raw = int_match.group("max")
            known.int_max = (
                None if max_raw == "None" else int(max_raw.removeprefix("Some(").removesuffix(")"))
            )
        elif float_match := _FLOAT_RANGE.search(kind_raw):
            known.float_min = float(float_match.group("min"))
            known.float_max = float(float_match.group("max"))
        elif enum_match := _ENUM_OPTIONS.search(kind_raw):
            known.enum_options = tuple(
                option.strip().strip('"')
                for option in enum_match.group("options").split(",")
                if option.strip()
            )
        registry[key] = known
    if not registry:
        raise SystemExit(f"error: no KnownSetting entries parsed from {settings_rs}")
    return registry


def convert_value(known: KnownSetting, raw: str) -> tuple[object, Optional[str]]:
    """Converts a legacy CSV cell into the typed JSON value for its kind.

    Returns ``(value, None)`` on success or ``(None, reason)`` on failure.
    The reason never echoes the raw value text.
    """
    if known.kind == "Bool":
        lowered = raw.strip().lower()
        if lowered in {"true", "1", "yes"}:
            return True, None
        if lowered in {"false", "0", "no"}:
            return False, None
        return None, "expected a boolean (true/false/1/0/yes/no)"
    if known.kind == "Int":
        try:
            number = int(raw.strip())
        except ValueError:
            return None, "expected an integer"
        if known.int_min is not None and number < known.int_min:
            return None, "value below the allowed minimum"
        if known.int_max is not None and number > known.int_max:
            return None, "value above the allowed maximum"
        return number, None
    if known.kind == "Float":
        try:
            number = float(raw.strip())
        except ValueError:
            return None, "expected a number"
        if known.float_min is not None and number < known.float_min:
            return None, "value below the allowed minimum"
        if known.float_max is not None and number > known.float_max:
            return None, "value above the allowed maximum"
        return number, None
    if known.kind == "Enum":
        if raw in known.enum_options:
            return raw, None
        return None, "value is not one of the allowed options"
    return raw, None


def build_document(
    csv_rows: list[dict[str, str]],
    registry: dict[str, KnownSetting],
    include_secrets: bool,
) -> tuple[dict[str, object], ConversionReport]:
    """Converts parsed CSV rows into the import document plus a report."""
    report = ConversionReport()
    settings: dict[str, object] = {}
    for row in csv_rows:
        key = (row.get("Key") or "").strip()
        if not key:
            continue
        raw = row.get("Value")
        if raw is None or raw == "":
            report.empty_values.append(key)
            continue
        known = registry.get(key)
        if known is None:
            report.unknown_keys.append(key)
            continue
        if known.key in NEVER_IMPORT_KEYS:
            report.never_imported.append(key)
            continue
        if known.secret and not include_secrets:
            report.secrets_omitted.append(key)
            continue
        value, error = convert_value(known, raw)
        if error is not None:
            report.errors.append(f"{key}: {error}")
            continue
        if len(json.dumps(value).encode("utf-8")) > MAX_SETTING_VALUE_BYTES:
            report.errors.append(f"{key}: value exceeds the {MAX_SETTING_VALUE_BYTES} byte limit")
            continue
        settings[key] = value
        (report.secrets_included if known.secret else report.converted).append(key)
    if len(settings) > MAX_IMPORT_KEYS:
        raise SystemExit(f"error: document would hold {len(settings)} keys (limit {MAX_IMPORT_KEYS})")
    document: dict[str, object] = {
        "format": CONFIG_EXPORT_FORMAT,
        "warning": CONFIG_EXPORT_WARNING,
        "exported_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "settings": dict(sorted(settings.items())),
    }
    return document, report


def read_csv_rows(csv_path: Path) -> list[dict[str, str]]:
    """Reads the legacy export CSV, failing closed on the expected columns."""
    with csv_path.open(newline="", encoding="utf-8-sig") as handle:
        reader = csv.DictReader(handle)
        fieldnames = reader.fieldnames or []
        if "Key" not in fieldnames or "Value" not in fieldnames:
            raise SystemExit("error: export CSV must have 'Key' and 'Value' columns")
        return [dict(row) for row in reader]


def summarize(report: ConversionReport) -> str:
    lines = [
        f"converted (non-secret): {len(report.converted)}",
        f"secrets included for re-entry: {len(report.secrets_included)}",
        f"secrets omitted (re-enter after import): {len(report.secrets_omitted)}",
    ]
    if report.secrets_omitted:
        lines.extend(f"  - {key}" for key in report.secrets_omitted)
    if report.never_imported:
        lines.append(f"never imported (regenerate instead): {len(report.never_imported)}")
        lines.extend(f"  - {key}" for key in report.never_imported)
    if report.unknown_keys:
        lines.append(f"unknown to the Rust registry (not emitted): {len(report.unknown_keys)}")
        lines.extend(f"  - {key}" for key in report.unknown_keys)
    if report.empty_values:
        lines.append(f"empty values skipped (defaults apply): {len(report.empty_values)}")
    if report.errors:
        lines.append(f"errors: {len(report.errors)}")
        lines.extend(f"  - {line}" for line in report.errors)
    return "\n".join(lines)


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Convert a legacy C# Melodee settings export (CSV with 'Key' and "
            "'Value' columns) into a melodee-config-export/v1 JSON document "
            "for 'melodee-cli config import'. Secret values are omitted "
            "unless --include-secrets is given; they are never printed."
        )
    )
    parser.add_argument("--csv", required=True, type=Path, help="path to the legacy settings export CSV")
    parser.add_argument(
        "--output",
        type=Path,
        help="output document path (default: alongside the CSV as <name>.melodee-config-import.json)",
    )
    parser.add_argument(
        "--include-secrets",
        action="store_true",
        help="include real secret values from the export (they will be stored on import)",
    )
    parser.add_argument("--force", action="store_true", help="overwrite an existing output file")
    args = parser.parse_args(argv)
    if not args.csv.is_file():
        parser.error(f"input CSV does not exist: {args.csv}")
    output = args.output or args.csv.with_name(args.csv.stem + ".melodee-config-import.json")
    if output.exists() and not args.force:
        parser.error(f"output already exists (pass --force to overwrite): {output}")

    repo_root = Path(__file__).resolve().parents[1]
    registry = parse_registry(repo_root / REGISTRY_RELATIVE_PATH)
    rows = read_csv_rows(args.csv)
    document, report = build_document(rows, registry, args.include_secrets)
    output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {output} ({len(document['settings'])} setting(s))")
    print(summarize(report), file=sys.stderr)
    return 1 if report.errors else 0


if __name__ == "__main__":
    sys.exit(main())
