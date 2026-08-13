#!/usr/bin/env python3
"""Build and validate the Phase 8 Rust implementation-evidence matrices.

The Phase 0 OpenSubsonic and Jellyfin inventories describe the frozen *legacy*
surfaces.  They do not say that an endpoint exists in Rust.  This generator
joins each inventory with its Phase 8 evidence registry so planning artifacts
cannot accidentally turn legacy capture coverage into an implementation claim.

OpenSubsonic evidence is keyed by endpoint name (the inventory is
endpoint-centric: each endpoint owns its plain and `.view` routes and GET+POST
methods).  Jellyfin evidence is keyed by method/path (the inventory is
route-centric).
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
OS_INVENTORY = ROOT / "compatibility" / "opensubsonic" / "contracts" / "route-inventory.json"
OS_EVIDENCE = (
    ROOT / "compatibility" / "opensubsonic" / "contracts" / "phase8-route-evidence.json"
)
OS_OUTPUT = ROOT / "design" / "agents" / "PHASE-8-OPENSUBSONIC-ROUTE-MATRIX.json"
JF_INVENTORY = ROOT / "compatibility" / "jellyfin" / "contracts" / "route-inventory.json"
JF_EVIDENCE = ROOT / "compatibility" / "jellyfin" / "contracts" / "phase8-route-evidence.json"
JF_OUTPUT = ROOT / "design" / "agents" / "PHASE-8-JELLYFIN-ROUTE-MATRIX.json"
SCHEMA_VERSION = 1

PENDING_STATUSES = {
    "implementation-pending",
    "negative-compatibility-pending",
}
EVIDENCED_STATUSES = {
    "implemented-and-differentially-tested",
    "approved-compatibility-normalization",
    "explicitly-deferred-by-accepted-scope-decision",
}
ALL_STATUSES = PENDING_STATUSES | EVIDENCED_STATUSES
PRD_ID_RE = re.compile(r"^PRD-[A-Z0-9-]+$")
RUST_TEST_RE = re.compile(r"^(?P<path>[^:]+\.rs)::(?P<name>[A-Za-z_][A-Za-z0-9_]*)$")

# Phase 8 obligations are the classified endpoint/route sets themselves.
OS_CLASSIFICATIONS = {"supported", "conditional", "gone", "not-implemented"}


class MatrixError(ValueError):
    """A Phase 8 evidence registry is inconsistent with the frozen inventory."""


def accepted_adr(authority: str, root: Path = ROOT) -> bool:
    path = root / authority
    if not path.is_file() or path.parent != root / "design" / "adr":
        return False
    return "- **Status:** Accepted" in path.read_text(encoding="utf-8")


def _require_nonempty_list(entry: dict[str, Any], field: str, key: str) -> list[str]:
    value = entry.get(field)
    if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
        raise MatrixError(f"{key}: {field} must be a non-empty string list")
    return value


def validate_rust_tests(tests: list[str], key: str, root: Path) -> None:
    for reference in tests:
        matched = RUST_TEST_RE.fullmatch(reference)
        if matched is None:
            raise MatrixError(f"{key}: invalid Rust test reference: {reference}")
        path = (root / matched.group("path")).resolve()
        try:
            path.relative_to(root.resolve())
        except ValueError as error:
            raise MatrixError(f"{key}: Rust test escapes repository: {reference}") from error
        if not path.is_file():
            raise MatrixError(f"{key}: Rust test file is missing: {reference}")
        source = _rust_code_without_comments_or_literals(path.read_text(encoding="utf-8"))
        name = re.escape(matched.group("name"))
        pattern = rf"(?P<attrs>(?:#\s*\[[^]]*\]\s*)+)(?:async\s+)?fn\s+{name}\b"
        declarations = list(re.finditer(pattern, source))
        is_runnable_test = any(
            re.search(r"#\s*\[\s*(?:tokio::)?test(?:\([^]]*\))?\s*\]", match["attrs"])
            and re.search(r"\bignore\b", match["attrs"]) is None
            for match in declarations
        )
        if not is_runnable_test:
            raise MatrixError(f"{key}: named Rust test is missing: {reference}")


def _rust_code_without_comments_or_literals(source: str) -> str:
    """Keep Rust structure while erasing places that can only mention fake tests."""
    output = list(source)
    index = 0
    block_depth = 0
    length = len(source)
    while index < length:
        if block_depth:
            if source.startswith("/*", index):
                output[index:index + 2] = "  "
                block_depth += 1
                index += 2
            elif source.startswith("*/", index):
                output[index:index + 2] = "  "
                block_depth -= 1
                index += 2
            else:
                if source[index] != "\n":
                    output[index] = " "
                index += 1
            continue
        if source.startswith("//", index):
            end = source.find("\n", index)
            end = length if end < 0 else end
            output[index:end] = " " * (end - index)
            index = end
            continue
        if source.startswith("/*", index):
            output[index:index + 2] = "  "
            block_depth = 1
            index += 2
            continue
        if source[index] == "'":
            end = _rust_char_literal_end(source, index)
            if end is not None:
                output[index:end] = " " * (end - index)
                index = end
                continue
        raw = re.match(r'r(?P<hashes>#{0,255})"', source[index:])
        if raw is not None:
            delimiter = '"' + raw.group("hashes")
            end = source.find(delimiter, index + raw.end())
            end = length if end < 0 else end + len(delimiter)
            for position in range(index, end):
                if source[position] != "\n":
                    output[position] = " "
            index = end
            continue
        if source[index] == '"':
            end = index + 1
            while end < length:
                if source[end] == "\\":
                    end += 2
                    continue
                end += 1
                if source[end - 1] == '"':
                    break
            for position in range(index, min(end, length)):
                if source[position] != "\n":
                    output[position] = " "
            index = end
            continue
        index += 1
    return "".join(output)


def _rust_char_literal_end(source: str, start: int) -> int | None:
    """Return the exclusive end of a Rust char literal, but never a lifetime."""
    value = start + 1
    if value >= len(source) or source[value] in "\r\n'":
        return None
    if source[value] != "\\":
        value += 1
    elif value + 1 >= len(source):
        return None
    elif source[value + 1] == "x":
        value += 4
    elif source[value + 1] == "u" and source.startswith("\\u{", value):
        closing_brace = source.find("}", value + 3)
        if closing_brace < 0:
            return None
        value = closing_brace + 1
    else:
        value += 2
    return value + 1 if value < len(source) and source[value] == "'" else None


def _validate_common_entry(
    entry: dict[str, Any],
    scenarios: list[str],
    key: str,
    *,
    root: Path = ROOT,
) -> None:
    status = entry.get("status")
    if status not in EVIDENCED_STATUSES:
        raise MatrixError(f"{key}: evidence status must be one of {sorted(EVIDENCED_STATUSES)}")

    authority = entry.get("authority")
    if not isinstance(authority, str) or not authority:
        raise MatrixError(f"{key}: authority is required")

    if status in {
        "approved-compatibility-normalization",
        "explicitly-deferred-by-accepted-scope-decision",
    } and not accepted_adr(authority, root):
        raise MatrixError(f"{key}: authority must be an accepted ADR: {authority}")

    if status == "explicitly-deferred-by-accepted-scope-decision":
        return

    requirements = _require_nonempty_list(entry, "requirements", key)
    invalid_requirements = [item for item in requirements if not PRD_ID_RE.fullmatch(item)]
    if invalid_requirements:
        raise MatrixError(f"{key}: invalid requirement IDs: {invalid_requirements}")
    rust_tests = _require_nonempty_list(entry, "rustTests", key)
    if len(rust_tests) != len(set(rust_tests)):
        raise MatrixError(f"{key}: rustTests contains duplicates")
    validate_rust_tests(rust_tests, key, root)

    legacy_evidence = _require_nonempty_list(entry, "legacyEvidence", key)
    unknown_evidence = sorted(set(legacy_evidence) - set(scenarios))
    if unknown_evidence:
        raise MatrixError(f"{key}: legacyEvidence is not attached here: {unknown_evidence}")


def build_opensubsonic_matrix(
    inventory: dict[str, Any],
    evidence: dict[str, Any],
    *,
    root: Path = ROOT,
) -> dict[str, Any]:
    if inventory.get("schemaVersion") != 1:
        raise MatrixError("unsupported opensubsonic inventory schemaVersion")
    if inventory.get("surface") != "opensubsonic":
        raise MatrixError("inventory surface must be opensubsonic")
    if evidence.get("schemaVersion") != 1:
        raise MatrixError("unsupported Phase 8 opensubsonic evidence schemaVersion")
    if evidence.get("surface") != "opensubsonic":
        raise MatrixError("evidence surface must be opensubsonic")
    if evidence.get("legacyBaselineCommit") != inventory.get("legacyBaselineCommit"):
        raise MatrixError("evidence legacyBaselineCommit must match the opensubsonic inventory")

    endpoints = inventory.get("endpoints")
    if not isinstance(endpoints, list) or inventory.get("endpointCount") != len(endpoints):
        raise MatrixError("opensubsonic inventory endpointCount must match its endpoint rows")
    evidence_rows = evidence.get("endpoints")
    if not isinstance(evidence_rows, list):
        raise MatrixError("Phase 8 opensubsonic evidence endpoints must be a list")

    legacy_endpoints = {str(endpoint["endpoint"]): endpoint for endpoint in endpoints}
    if len(legacy_endpoints) != len(endpoints):
        raise MatrixError("opensubsonic inventory contains duplicate endpoint names")
    unknown_classification = sorted(
        {str(e["classification"]) for e in endpoints} - OS_CLASSIFICATIONS
    )
    if unknown_classification:
        raise MatrixError(f"unknown opensubsonic classifications: {unknown_classification}")

    evidence_by_name: dict[str, dict[str, Any]] = {}
    for entry in evidence_rows:
        name = entry.get("endpoint")
        if not isinstance(name, str) or not name:
            raise MatrixError("opensubsonic evidence row is missing an endpoint name")
        if name in evidence_by_name:
            raise MatrixError(f"duplicate Phase 8 opensubsonic evidence row: {name}")
        legacy = legacy_endpoints.get(name)
        if legacy is None:
            raise MatrixError(f"opensubsonic evidence references an unknown endpoint: {name}")
        _validate_common_entry(entry, list(legacy.get("scenarios", [])), name, root=root)
        evidence_by_name[name] = entry

    rows: list[dict[str, Any]] = []
    for identifier, legacy in enumerate(endpoints, start=1):
        name = str(legacy["endpoint"])
        proof = evidence_by_name.get(name)
        negative = legacy["classification"] in {"gone", "not-implemented"}
        if proof is None:
            status = (
                "negative-compatibility-pending" if negative else "implementation-pending"
            )
            rationale = (
                f"The legacy endpoint answers HTTP {legacy['expectedUnsupportedStatus']} by "
                "contract. Phase 8 still needs a Rust route-level negative compatibility test."
                if negative
                else "The endpoint is a captured legacy obligation with no accepted Rust "
                "differential evidence yet."
            )
            proof_fields: dict[str, Any] = {
                "authority": None,
                "requirements": [],
                "legacyEvidence": [],
                "rustTests": [],
            }
        else:
            status = proof["status"]
            rationale = str(proof.get("rationale", "Accepted endpoint evidence is registered."))
            proof_fields = {
                "authority": proof["authority"],
                "requirements": proof.get("requirements", []),
                "legacyEvidence": proof.get("legacyEvidence", []),
                "rustTests": proof.get("rustTests", []),
            }

        rows.append(
            {
                "id": identifier,
                "endpoint": name,
                "routes": legacy["routes"],
                "methods": legacy["methods"],
                "controller": legacy["controller"],
                "legacyContract": {
                    "classification": legacy["classification"],
                    "prerequisite": legacy["prerequisite"],
                    "expectedUnsupportedStatus": legacy["expectedUnsupportedStatus"],
                    "isMelodeeExtension": legacy["isMelodeeExtension"],
                    "scenarios": legacy["scenarios"],
                },
                "phase8": {
                    "status": status,
                    "rationale": rationale,
                    **proof_fields,
                },
            }
        )

    counts = Counter(row["phase8"]["status"] for row in rows)
    return {
        "schemaVersion": SCHEMA_VERSION,
        "surface": "opensubsonic",
        "legacyBaselineCommit": inventory["legacyBaselineCommit"],
        "sources": [
            "compatibility/opensubsonic/contracts/route-inventory.json",
            "compatibility/opensubsonic/contracts/phase8-route-evidence.json",
        ],
        "endpointCount": len(rows),
        "legacyCaptureSummary": inventory["summary"],
        "phase8StatusCounts": {status: counts.get(status, 0) for status in sorted(ALL_STATUSES)},
        "completedEndpointCount": sum(
            counts[status]
            for status in EVIDENCED_STATUSES - {"explicitly-deferred-by-accepted-scope-decision"}
        ),
        "pendingEndpointCount": sum(counts[status] for status in PENDING_STATUSES),
        "deferredEndpointCount": counts["explicitly-deferred-by-accepted-scope-decision"],
        "invariants": [
            "Legacy capture is contract evidence, never evidence that a Rust endpoint is implemented.",
            "Every classified legacy OpenSubsonic endpoint is a Phase 8 obligation unless an accepted ADR explicitly defers it.",
            "Gone (410) and not-implemented (501) endpoints are implemented negative behavior and need Rust route tests asserting the exact captured status.",
            "Compatibility normalizations and deferrals require an accepted ADR named by the endpoint evidence row.",
        ],
        "endpoints": rows,
    }


def build_jellyfin_matrix(
    inventory: dict[str, Any],
    evidence: dict[str, Any],
    *,
    root: Path = ROOT,
) -> dict[str, Any]:
    if inventory.get("schemaVersion") != 1:
        raise MatrixError("unsupported jellyfin inventory schemaVersion")
    if inventory.get("surface") != "jellyfin":
        raise MatrixError("inventory surface must be jellyfin")
    if evidence.get("schemaVersion") != 1:
        raise MatrixError("unsupported Phase 8 jellyfin evidence schemaVersion")
    if evidence.get("surface") != "jellyfin":
        raise MatrixError("evidence surface must be jellyfin")
    if evidence.get("legacyBaselineCommit") != inventory.get("legacyBaselineCommit"):
        raise MatrixError("evidence legacyBaselineCommit must match the jellyfin inventory")

    routes = inventory.get("routes")
    if not isinstance(routes, list) or inventory.get("routeCount") != len(routes):
        raise MatrixError("jellyfin inventory routeCount must match its route rows")
    evidence_rows = evidence.get("routes")
    if not isinstance(evidence_rows, list):
        raise MatrixError("Phase 8 jellyfin evidence routes must be a list")

    def route_key(route: dict[str, Any]) -> tuple[str, str]:
        return str(route["method"]).upper(), str(route["path"])

    legacy_routes = {route_key(route): route for route in routes}
    if len(legacy_routes) != len(routes):
        raise MatrixError("jellyfin inventory contains duplicate method/path rows")

    evidence_by_key: dict[tuple[str, str], dict[str, Any]] = {}
    for entry in evidence_rows:
        key = route_key(entry)
        if key in evidence_by_key:
            raise MatrixError(f"duplicate Phase 8 jellyfin evidence row: {key[0]} {key[1]}")
        legacy = legacy_routes.get(key)
        if legacy is None:
            raise MatrixError(
                f"jellyfin evidence references an unknown route: {key[0]} {key[1]}"
            )
        _validate_common_entry(
            entry, list(legacy.get("scenarios", [])), f"{key[0]} {key[1]}", root=root
        )
        evidence_by_key[key] = entry

    rows: list[dict[str, Any]] = []
    for identifier, legacy in enumerate(routes, start=1):
        key = route_key(legacy)
        proof = evidence_by_key.get(key)
        if proof is None:
            status = "implementation-pending"
            rationale = (
                "The route is a captured legacy obligation with no accepted Rust "
                "differential evidence yet."
            )
            proof_fields = {
                "authority": None,
                "requirements": [],
                "legacyEvidence": [],
                "rustTests": [],
            }
        else:
            status = proof["status"]
            rationale = str(proof.get("rationale", "Accepted route evidence is registered."))
            proof_fields = {
                "authority": proof["authority"],
                "requirements": proof.get("requirements", []),
                "legacyEvidence": proof.get("legacyEvidence", []),
                "rustTests": proof.get("rustTests", []),
            }

        rows.append(
            {
                "id": identifier,
                "method": legacy["method"],
                "path": legacy["path"],
                "clientVisiblePath": legacy["clientVisiblePath"],
                "controller": legacy["controller"],
                "action": legacy["action"],
                "legacyContract": {
                    "implementation": legacy["implementation"],
                    "scenarios": legacy["scenarios"],
                },
                "phase8": {
                    "status": status,
                    "rationale": rationale,
                    **proof_fields,
                },
            }
        )

    counts = Counter(row["phase8"]["status"] for row in rows)
    return {
        "schemaVersion": SCHEMA_VERSION,
        "surface": "jellyfin",
        "legacyBaselineCommit": inventory["legacyBaselineCommit"],
        "sources": [
            "compatibility/jellyfin/contracts/route-inventory.json",
            "compatibility/jellyfin/contracts/phase8-route-evidence.json",
        ],
        "routeCount": len(rows),
        "legacyCaptureSummary": {
            "routeCount": inventory["routeCount"],
            "coveredCount": inventory["coveredCount"],
        },
        "phase8StatusCounts": {status: counts.get(status, 0) for status in sorted(ALL_STATUSES)},
        "completedRouteCount": sum(
            counts[status]
            for status in EVIDENCED_STATUSES - {"explicitly-deferred-by-accepted-scope-decision"}
        ),
        "pendingRouteCount": sum(counts[status] for status in PENDING_STATUSES),
        "deferredRouteCount": counts["explicitly-deferred-by-accepted-scope-decision"],
        "invariants": [
            "Legacy capture is contract evidence, never evidence that a Rust route is implemented.",
            "Every captured Jellyfin music route is a Phase 8 obligation unless an accepted ADR explicitly defers it.",
            "Non-music Jellyfin surfaces stay intentionally absent (plain 404); no evidence row may claim them.",
            "Compatibility normalizations and deferrals require an accepted ADR named by the route evidence row.",
        ],
        "routes": rows,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Fail if a committed matrix differs.")
    args = parser.parse_args()

    plans = [
        ("opensubsonic", OS_INVENTORY, OS_EVIDENCE, OS_OUTPUT, build_opensubsonic_matrix),
        ("jellyfin", JF_INVENTORY, JF_EVIDENCE, JF_OUTPUT, build_jellyfin_matrix),
    ]
    drifted = False
    for name, inventory_path, evidence_path, output_path, builder in plans:
        inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
        evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
        matrix = builder(inventory, evidence)
        rendered = json.dumps(matrix, indent=2, ensure_ascii=False) + "\n"
        total = len(matrix.get("endpoints", matrix.get("routes", [])))
        complete = matrix.get("completedEndpointCount", matrix.get("completedRouteCount", 0))
        pending = matrix.get("pendingEndpointCount", matrix.get("pendingRouteCount", 0))
        if args.check:
            if not output_path.exists() or output_path.read_text(encoding="utf-8") != rendered:
                print(f"DRIFT: {output_path.relative_to(ROOT)}", file=sys.stderr)
                drifted = True
                continue
            print(f"Phase 8 {name} route matrix up to date ({complete} complete, {pending} pending)")
        else:
            output_path.parent.mkdir(parents=True, exist_ok=True)
            output_path.write_text(rendered, encoding="utf-8")
            print(f"Phase 8 {name} route matrix: {total} rows -> {output_path.relative_to(ROOT)}")
            for status, count in matrix["phase8StatusCounts"].items():
                print(f"  {status}: {count}")
    if drifted:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
