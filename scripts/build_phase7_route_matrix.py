#!/usr/bin/env python3
"""Build and validate the Phase 7 Rust implementation-evidence matrix.

The Phase 0 native inventory describes the frozen *legacy* surface.  It does
not say that a route exists in Rust.  This generator joins that inventory with
the Phase 7 evidence registry so planning artifacts cannot accidentally turn
legacy capture coverage into an implementation claim.
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
INVENTORY = ROOT / "compatibility" / "melodee-api-v1" / "contracts" / "route-inventory.json"
EVIDENCE = (
    ROOT / "compatibility" / "melodee-api-v1" / "contracts" / "phase7-route-evidence.json"
)
OUTPUT = ROOT / "design" / "agents" / "PHASE-7-ROUTE-MATRIX.json"
SCHEMA_VERSION = 2

PENDING_STATUSES = {
    "implementation-pending",
    "negative-compatibility-pending",
}
EVIDENCED_STATUSES = {
    "implemented-and-differentially-tested",
    "preserved-intentional-legacy-absence",
    "approved-compatibility-normalization",
    "explicitly-deferred-by-accepted-scope-decision",
}
ALL_STATUSES = PENDING_STATUSES | EVIDENCED_STATUSES
PRD_ID_RE = re.compile(r"^PRD-[A-Z0-9-]+$")
RUST_TEST_RE = re.compile(r"^(?P<path>[^:]+\.rs)::(?P<name>[A-Za-z_][A-Za-z0-9_]*)$")


class MatrixError(ValueError):
    """The Phase 7 evidence registry is inconsistent with the frozen inventory."""


def route_key(route: dict[str, Any]) -> tuple[str, str]:
    return str(route["method"]).upper(), str(route["path"])


def accepted_adr(authority: str, root: Path = ROOT) -> bool:
    path = root / authority
    if not path.is_file() or path.parent != root / "design" / "adr":
        return False
    return "- **Status:** Accepted" in path.read_text(encoding="utf-8")


def _require_nonempty_list(entry: dict[str, Any], field: str, key: tuple[str, str]) -> list[str]:
    value = entry.get(field)
    if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
        raise MatrixError(f"{key[0]} {key[1]}: {field} must be a non-empty string list")
    return value


def validate_rust_tests(tests: list[str], key: tuple[str, str], root: Path) -> None:
    for reference in tests:
        matched = RUST_TEST_RE.fullmatch(reference)
        if matched is None:
            raise MatrixError(f"{key[0]} {key[1]}: invalid Rust test reference: {reference}")
        path = (root / matched.group("path")).resolve()
        try:
            path.relative_to(root.resolve())
        except ValueError as error:
            raise MatrixError(f"{key[0]} {key[1]}: Rust test escapes repository: {reference}") from error
        if not path.is_file():
            raise MatrixError(f"{key[0]} {key[1]}: Rust test file is missing: {reference}")
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
            raise MatrixError(f"{key[0]} {key[1]}: named Rust test is missing: {reference}")


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


def validate_evidence_entry(
    entry: dict[str, Any],
    legacy: dict[str, Any],
    *,
    root: Path = ROOT,
) -> None:
    key = route_key(entry)
    status = entry.get("status")
    if status not in EVIDENCED_STATUSES:
        raise MatrixError(
            f"{key[0]} {key[1]}: evidence status must be one of {sorted(EVIDENCED_STATUSES)}"
        )

    authority = entry.get("authority")
    if not isinstance(authority, str) or not authority:
        raise MatrixError(f"{key[0]} {key[1]}: authority is required")

    if status in {
        "approved-compatibility-normalization",
        "explicitly-deferred-by-accepted-scope-decision",
        "preserved-intentional-legacy-absence",
    } and not accepted_adr(authority, root):
        raise MatrixError(f"{key[0]} {key[1]}: authority must be an accepted ADR: {authority}")

    if status == "explicitly-deferred-by-accepted-scope-decision":
        return

    requirements = _require_nonempty_list(entry, "requirements", key)
    invalid_requirements = [item for item in requirements if not PRD_ID_RE.fullmatch(item)]
    if invalid_requirements:
        raise MatrixError(f"{key[0]} {key[1]}: invalid requirement IDs: {invalid_requirements}")
    rust_tests = _require_nonempty_list(entry, "rustTests", key)
    if len(rust_tests) != len(set(rust_tests)):
        raise MatrixError(f"{key[0]} {key[1]}: rustTests contains duplicates")
    validate_rust_tests(rust_tests, key, root)

    legacy_evidence = _require_nonempty_list(entry, "legacyEvidence", key)
    scenarios = set(legacy.get("scenarios", []))
    unknown_evidence = sorted(set(legacy_evidence) - scenarios)
    if unknown_evidence:
        raise MatrixError(
            f"{key[0]} {key[1]}: legacyEvidence is not attached to this route: {unknown_evidence}"
        )

    excluded = legacy.get("implementation") == "excluded-from-build"
    if status == "preserved-intentional-legacy-absence" and not excluded:
        raise MatrixError(f"{key[0]} {key[1]}: only legacy-excluded routes can preserve absence")
    if status in {
        "implemented-and-differentially-tested",
        "approved-compatibility-normalization",
    } and excluded:
        raise MatrixError(f"{key[0]} {key[1]}: legacy-excluded route cannot claim implemented parity")


def build_matrix(
    inventory: dict[str, Any],
    evidence: dict[str, Any],
    *,
    root: Path = ROOT,
) -> dict[str, Any]:
    if inventory.get("schemaVersion") != 1:
        raise MatrixError("unsupported native inventory schemaVersion")
    if inventory.get("surface") != "melodee-api-v1":
        raise MatrixError("inventory surface must be melodee-api-v1")
    if evidence.get("schemaVersion") != 1:
        raise MatrixError("unsupported Phase 7 evidence schemaVersion")
    if evidence.get("surface") != "melodee-api-v1":
        raise MatrixError("evidence surface must be melodee-api-v1")
    if evidence.get("legacyBaselineCommit") != inventory.get("legacyBaselineCommit"):
        raise MatrixError("evidence legacyBaselineCommit must match the native inventory")

    inventory_rows = inventory.get("routes")
    if not isinstance(inventory_rows, list) or inventory.get("routeCount") != len(inventory_rows):
        raise MatrixError("native inventory routeCount must match its route rows")
    evidence_rows = evidence.get("routes")
    if not isinstance(evidence_rows, list):
        raise MatrixError("Phase 7 evidence routes must be a list")

    cross_surface_rows = inventory.get("crossSurfaceDependencies", [])
    cross_surface_evidence = evidence.get("crossSurfaceDependencies", [])
    if not isinstance(cross_surface_rows, list) or not isinstance(cross_surface_evidence, list):
        raise MatrixError("cross-surface dependencies/evidence must be lists")
    cross_by_key = {route_key(route): route for route in cross_surface_rows}
    cross_proof_by_key: dict[tuple[str, str], dict[str, Any]] = {}
    for entry in cross_surface_evidence:
        key = route_key(entry)
        if key in cross_proof_by_key:
            raise MatrixError(f"duplicate cross-surface evidence row: {key[0]} {key[1]}")
        legacy = cross_by_key.get(key)
        if legacy is None:
            raise MatrixError(f"cross-surface evidence references an unknown route: {key[0]} {key[1]}")
        requirements = _require_nonempty_list(entry, "requirements", key)
        if any(not PRD_ID_RE.fullmatch(item) for item in requirements):
            raise MatrixError(f"{key[0]} {key[1]}: invalid cross-surface requirement ID")
        tests = _require_nonempty_list(entry, "rustTests", key)
        if len(tests) != len(set(tests)):
            raise MatrixError(f"{key[0]} {key[1]}: rustTests contains duplicates")
        validate_rust_tests(tests, key, root)
        evidence_names = _require_nonempty_list(entry, "legacyEvidence", key)
        unknown = sorted(set(evidence_names) - set(legacy.get("scenarios", [])))
        if unknown:
            raise MatrixError(f"{key[0]} {key[1]}: unknown cross-surface evidence: {unknown}")
        cross_proof_by_key[key] = entry

    legacy_routes = {route_key(route): route for route in inventory_rows}
    if len(legacy_routes) != len(inventory_rows):
        raise MatrixError("native inventory contains duplicate method/path rows")

    evidence_by_key: dict[tuple[str, str], dict[str, Any]] = {}
    for entry in evidence_rows:
        key = route_key(entry)
        if key in evidence_by_key:
            raise MatrixError(f"duplicate Phase 7 evidence row: {key[0]} {key[1]}")
        legacy = legacy_routes.get(key)
        if legacy is None:
            raise MatrixError(f"Phase 7 evidence references an unknown route: {key[0]} {key[1]}")
        validate_evidence_entry(entry, legacy, root=root)
        evidence_by_key[key] = entry

    routes: list[dict[str, Any]] = []
    for identifier, legacy in enumerate(inventory_rows, start=1):
        key = route_key(legacy)
        proof = evidence_by_key.get(key)
        excluded = legacy["implementation"] == "excluded-from-build"
        if proof is None:
            status = "negative-compatibility-pending" if excluded else "implementation-pending"
            rationale = (
                "The legacy controller was excluded from its build. Phase 7 still needs a legacy "
                "negative capture and a Rust route-level negative compatibility test."
                if excluded
                else "The route is a compiled legacy native-v1 obligation with no accepted Rust "
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

        routes.append(
            {
                "id": identifier,
                "method": legacy["method"],
                "path": legacy["path"],
                "manifestPath": legacy["manifestPath"],
                "controller": legacy["controller"],
                "action": legacy["action"],
                "legacyContract": {
                    "implementation": legacy["implementation"],
                    "captureStatus": legacy["parityStatus"],
                    "inOpenApi": legacy["inOpenApi"],
                    "usedBy": legacy["usedBy"],
                    "declaredScenarios": legacy["declaredScenarios"],
                    "scenarios": legacy["scenarios"],
                },
                "phase7": {
                    "status": status,
                    "rationale": rationale,
                    **proof_fields,
                },
            }
        )

    counts = Counter(route["phase7"]["status"] for route in routes)
    cross_surface = []
    for legacy in cross_surface_rows:
        proof = cross_proof_by_key.get(route_key(legacy))
        cross_surface.append(
            {
                **legacy,
                "phase7": {
                    "status": "implemented-and-differentially-tested" if proof else "implementation-pending",
                    "authority": None if proof is None else proof.get("authority"),
                    "requirements": [] if proof is None else proof["requirements"],
                    "legacyEvidence": [] if proof is None else proof["legacyEvidence"],
                    "rustTests": [] if proof is None else proof["rustTests"],
                    "rationale": (
                        "Native clients depend on this route, but no Rust implementation evidence is registered."
                        if proof is None
                        else proof.get("rationale", "Cross-surface implementation evidence is registered.")
                    ),
                },
            }
        )

    return {
        "schemaVersion": SCHEMA_VERSION,
        "surface": "melodee-api-v1",
        "legacyBaselineCommit": inventory["legacyBaselineCommit"],
        "sources": [
            "compatibility/melodee-api-v1/contracts/route-inventory.json",
            "compatibility/melodee-api-v1/contracts/phase7-route-evidence.json",
        ],
        "routeCount": len(routes),
        "legacyCaptureSummary": inventory["summary"],
        "phase7StatusCounts": {status: counts.get(status, 0) for status in sorted(ALL_STATUSES)},
        "completedRouteCount": sum(counts[status] for status in EVIDENCED_STATUSES - {"explicitly-deferred-by-accepted-scope-decision"}),
        "pendingRouteCount": sum(counts[status] for status in PENDING_STATUSES),
        "deferredRouteCount": counts["explicitly-deferred-by-accepted-scope-decision"],
        "invariants": [
            "Legacy capture is contract evidence, never evidence that a Rust route is implemented.",
            "Every compiled legacy native-v1 route is a Phase 7 obligation unless an accepted ADR explicitly defers it.",
            "Every excluded legacy route needs both legacy negative capture and a Rust negative compatibility test before absence is proven.",
            "Compatibility normalizations and deferrals require an accepted ADR named by the route evidence row.",
        ],
        "routes": routes,
        "crossSurfaceDependencies": cross_surface,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Fail if the committed matrix differs.")
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()

    inventory = json.loads(INVENTORY.read_text(encoding="utf-8"))
    evidence = json.loads(EVIDENCE.read_text(encoding="utf-8"))
    matrix = build_matrix(inventory, evidence)
    rendered = json.dumps(matrix, indent=2, ensure_ascii=False) + "\n"

    if args.check:
        if not args.output.exists() or args.output.read_text(encoding="utf-8") != rendered:
            print(f"DRIFT: {args.output.relative_to(ROOT)}", file=sys.stderr)
            raise SystemExit(1)
        print(
            "Phase 7 route matrix up to date "
            f"({matrix['completedRouteCount']} complete, {matrix['pendingRouteCount']} pending, "
            f"{matrix['deferredRouteCount']} deferred)"
        )
        return

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(rendered, encoding="utf-8")
    print(f"Phase 7 route matrix: {len(matrix['routes'])} routes -> {args.output.relative_to(ROOT)}")
    for status, count in matrix["phase7StatusCounts"].items():
        print(f"  {status}: {count}")


if __name__ == "__main__":
    main()
