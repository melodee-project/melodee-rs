#!/usr/bin/env python3
"""Validate Admin UI locale catalogs and their generated key-coverage manifest."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCALES = ROOT / "admin-ui/src/i18n/locales"
COVERAGE = ROOT / "admin-ui/src/i18n/coverage.json"


def catalog_coverage(directory: Path) -> dict[str, dict[str, int]]:
    """Measure key availability, not linguistic translation quality."""
    catalogs = {}
    for path in sorted(directory.glob("*.json")):
        value = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(value, dict):
            raise ValueError(f"{path.name}: expected a key/value object")
        catalogs[path.stem] = value
    if "en-US" not in catalogs:
        raise ValueError("source catalog en-US.json is missing")
    source = set(catalogs["en-US"])
    if not source:
        raise ValueError("source catalog is empty")
    result = {}
    for culture, catalog in catalogs.items():
        unknown = set(catalog) - source
        if unknown:
            raise ValueError(f"{culture}: unknown source keys: {', '.join(sorted(unknown))}")
        for key, value in catalog.items():
            if not isinstance(value, str) or not value.strip():
                raise ValueError(f"{culture}: {key} must be a nonempty string; omit to fall back")
        result[culture] = {"present": len(catalog), "total": len(source)}
    return result


def validate_coverage(directory: Path, manifest: Path, *, write: bool = False) -> dict:
    coverage = catalog_coverage(directory)
    expected = json.dumps(coverage, ensure_ascii=False, indent=2) + "\n"
    if write:
        manifest.write_text(expected, encoding="utf-8")
    elif not manifest.exists() or manifest.read_text(encoding="utf-8") != expected:
        raise ValueError("locale coverage is stale; run python3 scripts/check_locales.py --write-coverage")
    return coverage


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write-coverage", action="store_true", help="regenerate the committed key-coverage manifest")
    args = parser.parse_args()
    try:
        coverage = validate_coverage(LOCALES, COVERAGE, write=args.write_coverage)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Locale validation failed: {error}\n")
    print("Locale key coverage (missing keys use English):")
    for culture, counts in coverage.items():
        present, total = counts["present"], counts["total"]
        print(f"  {culture}: {present}/{total} ({100 * present / total:.1f}%)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
