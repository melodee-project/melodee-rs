#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class Difference:
    scenario: str
    field: str
    expected: Any
    actual: Any


def metadata_by_name(directory: Path) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for path in sorted(directory.glob("*.metadata.json")):
        metadata = json.loads(path.read_text(encoding="utf-8"))
        result[str(metadata["name"])] = metadata
    return result


def compare(expected_dir: Path, actual_dir: Path) -> list[Difference]:
    expected = metadata_by_name(expected_dir)
    actual = metadata_by_name(actual_dir)
    differences: list[Difference] = []

    for missing in sorted(expected.keys() - actual.keys()):
        differences.append(Difference(missing, "scenario", "present", "missing"))
    for extra in sorted(actual.keys() - expected.keys()):
        differences.append(Difference(extra, "scenario", "absent", "extra"))

    for name in sorted(expected.keys() & actual.keys()):
        left = expected[name]
        right = actual[name]
        comparisons = {
            "response.status": (left["response"]["status"], right["response"]["status"]),
            "response.headers": (left["response"]["headers"], right["response"]["headers"]),
            "response.sha256": (left["response"]["sha256"], right["response"]["sha256"]),
        }
        for field, (expected_value, actual_value) in comparisons.items():
            if expected_value != actual_value:
                differences.append(Difference(name, field, expected_value, actual_value))
    return differences


def main() -> None:
    parser = argparse.ArgumentParser(description="Compare two sanitized capture directories")
    parser.add_argument("expected", type=Path)
    parser.add_argument("actual", type=Path)
    args = parser.parse_args()

    differences = compare(args.expected, args.actual)
    if not differences:
        print("Capture metadata and body hashes match exactly.")
        return

    for item in differences:
        print(f"{item.scenario}: {item.field}\n  expected: {item.expected}\n  actual:   {item.actual}")
    raise SystemExit(1)


if __name__ == "__main__":
    main()
