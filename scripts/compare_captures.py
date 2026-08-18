#!/usr/bin/env python3
"""Compare two sanitized capture directories with volatile-field normalization.

Phase 0 differential-testing tool (FIRST-10-ISSUES.md Issue 4). Comparison
rules:

- Only fields declared volatile by the scenario's ``normalize`` block are
  masked. Everything else must match exactly.
- JSON bodies are compared structurally: object key order is ignored, but
  null-vs-omitted distinctions and array ordering are contract and preserved.
- Response headers are compared after dropping scenario-declared volatile
  headers.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import numbers
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

MASK = "<volatile>"
MAX_DIFFERENCES = 50


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


_SEGMENT_RE = re.compile(r"\.([^\.\[\]]+)|\[(\*|\d+)\]")
_REQUEST_VARIABLE_RE = re.compile(r"\{\{[^{}]+\}\}")


def parse_json_path(path: str) -> list[str | int]:
    """Parse the supported JSON-path subset: $.a.b, $.a[*].b, $.a[0].b."""
    if not path.startswith("$"):
        raise ValueError(f"json path must start with '$': {path}")
    segments: list[str | int] = []
    position = 1
    while position < len(path):
        match = _SEGMENT_RE.match(path, position)
        if not match:
            raise ValueError(f"unsupported json path syntax at {position}: {path}")
        key, index = match.groups()
        if key is not None:
            segments.append(key)
        elif index == "*":
            segments.append("*")
        else:
            segments.append(int(index))
        position = match.end()
    return segments


def mask_json_path(document: Any, segments: list[str | int]) -> Any:
    """Return a copy of ``document`` with the value at ``segments`` replaced by
    MASK. Missing paths are tolerated (the field may be absent in a variant)."""
    if not segments:
        return MASK
    head, rest = segments[0], segments[1:]
    if isinstance(document, dict):
        if head not in document:
            return document
        return {**document, head: mask_json_path(document[head], rest)}
    if isinstance(document, list):
        if head == "*":
            return [mask_json_path(item, rest) for item in document]
        if isinstance(head, int) and 0 <= head < len(document):
            return [
                mask_json_path(item, rest) if index == head else item
                for index, item in enumerate(document)
            ]
        return document
    return document


def normalize_json(document: Any, mask_paths: list[str]) -> Any:
    for path in mask_paths:
        document = mask_json_path(document, parse_json_path(path))
    return document


def normalize_request_url(url: str, template: str | None) -> str:
    """Replace only scenario-declared request variables with one marker.

    Flow-created IDs and signed stream URLs are intentionally different on
    every isolated replay. The scenario template is the authority for which
    URL portions are variable; static route text remains compared exactly.
    """
    if not template or not _REQUEST_VARIABLE_RE.search(template):
        return url
    pieces = _REQUEST_VARIABLE_RE.split(template)
    has_masked_origin = template.startswith("/")
    pattern = "^(?P<prefix>.*?)" if has_masked_origin else "^"
    for index, piece in enumerate(pieces):
        pattern += re.escape(piece)
        if index < len(pieces) - 1:
            pattern += ".*?"
    pattern += "$"
    if re.fullmatch(pattern, url, flags=re.DOTALL) is None:
        return url
    normalized = _REQUEST_VARIABLE_RE.sub("<scenario-variable>", template)
    if has_masked_origin:
        match = re.fullmatch(pattern, url, flags=re.DOTALL)
        assert match is not None
        return f"{match.group('prefix')}{normalized}"
    return normalized


def _short(value: Any) -> str:
    text = json.dumps(value, ensure_ascii=False)
    return text if len(text) <= 120 else text[:117] + "..."


def normalize_structure_checks(checks: Any) -> Any:
    """Remove replay-time expiry instants from transport-shape evidence.

    A stream token's signed expiry is intentionally minted at capture time;
    the structural validator still compares its validity, binding, HMAC
    shape, and relative lifetime. The absolute instant is not a wire field
    and differs whenever the two servers are replayed at different times.
    """
    if not isinstance(checks, dict):
        return checks
    normalized = json.loads(json.dumps(checks))
    for result in normalized.values():
        if isinstance(result, dict) and result.get("type") == "streamAuthToken":
            result.pop("expiresAt", None)
    return normalized


def diff_json(expected: Any, actual: Any, path: str = "$") -> list[tuple[str, Any, Any]]:
    """Structural diff. Key order-insensitive; array order, types, and
    null-vs-omitted distinctions are significant."""
    differences: list[tuple[str, Any, Any]] = []
    # JSON has one number type. Python's decoder materializes integral and
    # fractional JSON numbers as different Python types, so 0 and 0.0 are
    # wire-equivalent and must not become a false compatibility mismatch.
    if (
        isinstance(expected, numbers.Number)
        and not isinstance(expected, bool)
        and isinstance(actual, numbers.Number)
        and not isinstance(actual, bool)
    ):
        return [] if expected == actual else [(path, expected, actual)]
    if type(expected) is not type(actual):
        return [(path, expected, actual)]
    if isinstance(expected, dict):
        for key in sorted(expected.keys() - actual.keys()):
            differences.append((f"{path}.{key}", expected[key], "<omitted>"))
        for key in sorted(actual.keys() - expected.keys()):
            differences.append((f"{path}.{key}", "<omitted>", actual[key]))
        for key in sorted(expected.keys() & actual.keys()):
            differences.extend(diff_json(expected[key], actual[key], f"{path}.{key}"))
        return differences
    if isinstance(expected, list):
        if len(expected) != len(actual):
            differences.append((f"{path}.length", len(expected), len(actual)))
        for index, (left, right) in enumerate(zip(expected, actual)):
            differences.extend(diff_json(left, right, f"{path}[{index}]"))
        return differences
    if expected != actual:
        differences.append((path, expected, actual))
    return differences


def _load_body(directory: Path, metadata: dict[str, Any]) -> bytes:
    return (directory / metadata["response"]["bodyFile"]).read_bytes()


def compare_scenario(
    name: str,
    expected_dir: Path,
    expected: dict[str, Any],
    actual_dir: Path,
    actual: dict[str, Any],
    request_template: str | None = None,
) -> list[Difference]:
    differences: list[Difference] = []

    # The request is part of the compatibility contract. A response-only
    # comparison could otherwise bless a handler registered under the wrong
    # method/path or a changed query/body shape.
    for field in ("method", "url", "auth", "headers", "jsonTemplate", "bodyTemplate"):
        left = expected.get("request", {}).get(field)
        right = actual.get("request", {}).get(field)
        if field == "url" and request_template:
            left = normalize_request_url(left, request_template)
            right = normalize_request_url(right, request_template)
        if left != right:
            differences.append(Difference(name, f"request.{field}", left, right))

    left_status = expected["response"]["status"]
    right_status = actual["response"]["status"]
    if left_status != right_status:
        differences.append(Difference(name, "response.status", left_status, right_status))

    # Only the baseline's policy is authoritative. Allowing the actual capture
    # to add masks could hide a Rust contract regression.
    normalize = expected.get("normalize") or {}
    if (actual.get("normalize") or {}) != normalize:
        differences.append(
            Difference(name, "normalize", normalize, actual.get("normalize") or {})
        )
    dropped = {h.lower() for h in normalize.get("dropHeaders", [])}
    left_headers = {
        k.lower(): v
        for k, v in expected["response"]["headers"].items()
        if k.lower() not in dropped
    }
    right_headers = {
        k.lower(): v
        for k, v in actual["response"]["headers"].items()
        if k.lower() not in dropped
    }
    if left_headers != right_headers:
        differences.append(Difference(name, "response.headers", left_headers, right_headers))

    mask_paths = list(normalize.get("maskJsonPaths", []))
    left_body = _load_body(expected_dir, expected)
    right_body = _load_body(actual_dir, actual)
    try:
        left_json = json.loads(left_body)
        right_json = json.loads(right_body)
    except (json.JSONDecodeError, UnicodeDecodeError):
        if left_body != right_body:
            left_digest = hashlib.sha256(left_body).hexdigest()
            right_digest = hashlib.sha256(right_body).hexdigest()
            differences.append(
                Difference(
                    name,
                    "response.body",
                    f"{len(left_body)} bytes sha256={left_digest}",
                    f"{len(right_body)} bytes sha256={right_digest}",
                )
            )
        return differences

    left_norm = normalize_json(left_json, mask_paths)
    right_norm = normalize_json(right_json, mask_paths)
    for path, left, right in diff_json(left_norm, right_norm):
        differences.append(Difference(name, f"response.body{path}", _short(left), _short(right)))

    left_checks = normalize_structure_checks(expected.get("structureChecks"))
    right_checks = normalize_structure_checks(actual.get("structureChecks"))
    if left_checks != right_checks:
        differences.append(Difference(name, "structureChecks", left_checks, right_checks))
    return differences


def compare(
    expected_dir: Path,
    actual_dir: Path,
    scenario_names: set[str] | None = None,
    request_templates: dict[str, str] | None = None,
) -> list[Difference]:
    expected = metadata_by_name(expected_dir)
    actual = metadata_by_name(actual_dir)
    differences: list[Difference] = []

    if scenario_names is not None:
        unknown = sorted(scenario_names - expected.keys())
        if unknown:
            return [Difference(name, "scenario", "present in baseline", "unknown") for name in unknown]
        expected = {name: value for name, value in expected.items() if name in scenario_names}
        actual = {name: value for name, value in actual.items() if name in scenario_names}

    for missing in sorted(expected.keys() - actual.keys()):
        differences.append(Difference(missing, "scenario", "present", "missing"))
    for extra in sorted(actual.keys() - expected.keys()):
        differences.append(Difference(extra, "scenario", "absent", "extra"))

    for name in sorted(expected.keys() & actual.keys()):
        differences.extend(
            compare_scenario(
                name,
                expected_dir,
                expected[name],
                actual_dir,
                actual[name],
                (request_templates or {}).get(name),
            )
        )
    return differences


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("expected", type=Path)
    parser.add_argument("actual", type=Path)
    parser.add_argument(
        "--scenario",
        action="append",
        dest="scenarios",
        help="Compare only this scenario name (repeatable); useful for an incremental route slice.",
    )
    args = parser.parse_args()

    differences = compare(args.expected, args.actual, set(args.scenarios) if args.scenarios else None)
    if not differences:
        print("Captures match after normalization.")
        return

    for item in differences[:MAX_DIFFERENCES]:
        print(f"{item.scenario}: {item.field}\n  expected: {item.expected}\n  actual:   {item.actual}")
    if len(differences) > MAX_DIFFERENCES:
        print(f"... and {len(differences) - MAX_DIFFERENCES} more differences")
    raise SystemExit(1)


if __name__ == "__main__":
    main()
