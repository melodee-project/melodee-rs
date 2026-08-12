#!/usr/bin/env python3
"""Build the unified native Melodee API v1 contract inventory.

FIRST-10-ISSUES.md Issue 5 / Phase 0 review item 2. Merges, per route:

- the static route manifest (method/path/controller/auth/build exclusion);
- presence in the published OpenAPI document;
- actual native-client usage (``native-client-usage.json``);
- compatibility scenario coverage (``compatibility/scenarios/*.json``).

The published OpenAPI document is one input, not the authority: live routes
missing from it (and routes missing at runtime because their controllers are
excluded from compilation) are first-class rows here. Output is deterministic
(no timestamps) so ``--check`` can gate drift.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
SURFACE_DIR = ROOT / "compatibility" / "melodee-api-v1"
SCENARIOS_DIR = ROOT / "compatibility" / "scenarios"
GOLDEN_DIR = SURFACE_DIR / "golden"
SCHEMA_VERSION = 1

UUID_RE = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
CAPTURE_VAR_RE = re.compile(r"\{\{[^}]+\}\}")
ROUTE_PARAM_RE = re.compile(r"\{[^}]*\}")
INTEGER_SEGMENT_RE = re.compile(r"(?<=/)\d+(?=/|$)")
PUBLIC_SHARE_SEGMENT_RE = re.compile(r"(?<=/api/v1/shares/public/)[^/?#]+")


def display_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def normalize_path(path: str) -> str:
    """Normalize a concrete or templated path to the {} parameter form."""
    path = path.split("?", 1)[0]
    path = path.split("#", 1)[0]
    path = UUID_RE.sub("{}", path)
    path = CAPTURE_VAR_RE.sub("{}", path)
    path = ROUTE_PARAM_RE.sub("{}", path)
    # Concrete integer parameters occur inside some captured paths (for
    # example /participants/1/ban), not only at the end. The frozen manifest
    # has no fixed all-numeric path segments; a regression test guards that
    # precondition so adding such a route cannot silently collapse identities.
    path = INTEGER_SEGMENT_RE.sub("{}", path)
    # Public share IDs are short opaque strings rather than UUIDs. No literal
    # child route exists beneath this prefix in the frozen manifest.
    path = PUBLIC_SHARE_SEGMENT_RE.sub("{}", path)
    # Literal trailing boolean parameters in client-style paths (for example
    # starred/{id}/true).
    path = re.sub(r"/(true|false)$", "/{}", path)
    return path


def normalize_scenario_path(path: str, method: str, manifest: dict) -> str:
    """Resolve concrete non-UUID values (slugs, periods, asset paths) against
    the frozen route templates before falling back to generic normalization."""
    request_path = path.split("?", 1)[0].split("#", 1)[0]
    candidates: list[str] = []
    for route in manifest["routes"]:
        if method not in route["methods"]:
            continue
        pattern = re.escape(str(route["path"]))
        pattern = re.sub(r"\\\{\\\*[^}]+\\\}", ".+", pattern)
        pattern = re.sub(r"\\\{[^}]+\\\}", "[^/]+", pattern)
        if re.fullmatch(pattern, request_path):
            candidates.append(normalize_path(str(route["path"])))
    if len(candidates) == 1:
        return candidates[0]
    return normalize_path(path)


def scenario_route_map(manifest: dict | None = None) -> dict[tuple[str, str], list[str]]:
    """Map (method, normalized path) -> scenario step names."""
    if manifest is None:
        manifest = json.loads(
            (SURFACE_DIR / "contracts" / "route-manifest.json").read_text(encoding="utf-8")
        )
    mapping: dict[tuple[str, str], list[str]] = {}
    for scenario_file in sorted(SCENARIOS_DIR.glob("*.json")):
        scenarios = json.loads(scenario_file.read_text(encoding="utf-8"))
        for scenario in scenarios:
            steps = scenario.get("flow") if isinstance(scenario.get("flow"), list) else [scenario]
            for index, step in enumerate(steps):
                name = f"{scenario['name']}/{step.get('name', f'step-{index + 1}')}" if steps is not [scenario] and isinstance(scenario.get("flow"), list) else str(scenario["name"])
                # A negative capture may need a concrete opaque path value
                # while still declaring which parameterized manifest route it
                # proves. `inventoryPath` is evidence metadata only; the
                # capture runner continues to request `path` verbatim.
                raw_path = str(step.get("inventoryPath", step.get("path", "")))
                method = str(step.get("method", "GET")).upper()
                if "{{streamUrl}}" in raw_path:
                    normalized = "/song/stream/{}/{}/{}"
                elif "/song/stream/" in raw_path:
                    normalized = "/song/stream/{}/{}/{}"
                elif raw_path.startswith("/images/"):
                    normalized = normalize_scenario_path(raw_path, method, manifest)
                    mapping.setdefault((method, normalized), []).append(name)
                    continue
                else:
                    normalized = normalize_scenario_path(raw_path, method, manifest)
                mapping.setdefault((method, normalized), []).append(name)
    return mapping


def captured_url_path(url: str) -> str:
    if url.startswith("{") and "}" in url:
        return url.split("}", 1)[1]
    parsed = urlsplit(url)
    return parsed.path + (f"?{parsed.query}" if parsed.query else "")


def promoted_golden_names(manifest: dict | None = None) -> set[str]:
    """Return scenario names backed by promoted metadata and response bodies."""
    names: set[str] = set()
    if manifest is None:
        manifest = json.loads(
            (SURFACE_DIR / "contracts" / "route-manifest.json").read_text(encoding="utf-8")
        )
    expected_commit = manifest["legacyBaselineCommit"]
    declared = scenario_route_map(manifest)
    declared_by_name: dict[str, set[tuple[str, str]]] = {}
    for key, scenario_names in declared.items():
        for scenario_name in scenario_names:
            declared_by_name.setdefault(scenario_name, set()).add(key)
    for metadata_path in sorted(GOLDEN_DIR.glob("*/*.metadata.json")):
        provenance_path = metadata_path.parent / "provenance.json"
        if not provenance_path.is_file():
            raise ValueError(
                f"promoted metadata has no provenance: {display_path(metadata_path)}"
            )
        provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
        if (
            provenance.get("schemaVersion") != 1
            or provenance.get("surface") != "melodee-api-v1"
            or provenance.get("legacyBaselineCommit") != expected_commit
            or not str(provenance.get("reproducedBy", "")).strip()
        ):
            raise ValueError(
                f"promoted provenance is invalid: {display_path(provenance_path)}"
            )
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        name = str(metadata["name"])
        if name in names:
            raise ValueError(f"duplicate promoted native scenario name: {name}")
        body_file = metadata.get("response", {}).get("bodyFile")
        body_path = metadata_path.parent / body_file if isinstance(body_file, str) else None
        if body_path is None or not body_path.is_file():
            raise ValueError(f"promoted metadata has no response body: {display_path(metadata_path)}")
        expected_hash = metadata.get("response", {}).get("sha256")
        if not isinstance(expected_hash, str) or hashlib.sha256(body_path.read_bytes()).hexdigest() != expected_hash:
            raise ValueError(f"promoted response hash mismatch: {display_path(metadata_path)}")
        expected_bytes = metadata.get("response", {}).get("bytes")
        if expected_bytes != body_path.stat().st_size:
            raise ValueError(f"promoted response byte count mismatch: {display_path(metadata_path)}")
        request = metadata.get("request", {})
        method = str(request.get("method", "")).upper()
        url = str(request.get("url", ""))
        if not method or not url:
            raise ValueError(f"promoted metadata has no request identity: {display_path(metadata_path)}")
        captured_path = captured_url_path(url)
        normalized = (
            "/song/stream/{}/{}/{}"
            if "/song/stream/" in captured_path
            else normalize_scenario_path(captured_path, method, manifest)
        )
        if (method, normalized) not in declared_by_name.get(name, set()):
            raise ValueError(
                f"promoted request method/path does not match scenario: {display_path(metadata_path)}"
            )
        names.add(name)
    return names


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=SURFACE_DIR / "contracts" / "route-inventory.json",
    )
    parser.add_argument("--check", action="store_true", help="Fail if the committed inventory differs.")
    args = parser.parse_args()

    manifest = json.loads((SURFACE_DIR / "contracts" / "route-manifest.json").read_text(encoding="utf-8"))
    openapi = json.loads((SURFACE_DIR / "contracts" / "openapi-v1.json").read_text(encoding="utf-8"))
    usage = json.loads((SURFACE_DIR / "contracts" / "native-client-usage.json").read_text(encoding="utf-8"))
    declared_scenarios = scenario_route_map(manifest)
    promoted_names = promoted_golden_names(manifest)

    openapi_paths: dict[tuple[str, str], None] = {}
    for path, ops in openapi.get("paths", {}).items():
        for method in ops:
            openapi_paths[(method.upper(), normalize_path(path))] = None

    client_paths: dict[tuple[str, str], list[dict[str, str]]] = {}
    for client in usage["clients"]:
        for route in client["routes"]:
            key = (route["method"].upper(), normalize_path(route["path"]))
            client_paths.setdefault(key, []).append(
                {"client": client["name"], "evidence": route["evidence"], "usedFor": route.get("usedFor", "")}
            )

    routes = []
    for route in manifest["routes"]:
        for method in route["methods"]:
            key = (method, normalize_path(route["path"]))
            used_by = client_paths.get(key, [])
            declared_names = declared_scenarios.get(key, [])
            scenario_names = [name for name in declared_names if name in promoted_names]
            if route.get("buildExcluded"):
                parity = "excluded-from-build"
            elif scenario_names:
                parity = "captured"
            elif used_by:
                parity = "client-used-uncaptured"
            elif key in openapi_paths:
                parity = "documented-uncaptured"
            else:
                parity = "undocumented-uncaptured"
            routes.append(
                {
                    "method": method,
                    "path": key[1],
                    "manifestPath": route["path"],
                    "controller": route["controller"],
                    "action": route["action"],
                    "auth": route["auth"],
                    "implementation": route["implementation"],
                    "inOpenApi": key in openapi_paths,
                    "usedBy": used_by,
                    "declaredScenarios": declared_names,
                    "scenarios": scenario_names,
                    "parityStatus": parity,
                }
            )

    # Clients never construct the stream URL; they consume the server-provided
    # self-authenticating streamUrl verbatim (see native-client-usage.json
    # opaqueServerUrls). Reflect that dependency on the stream route itself.
    stream_consumers = [
        {
            "client": client["name"],
            "evidence": next(
                (entry["consumedBy"] for entry in client.get("opaqueServerUrls", []) if entry["field"] == "Song.streamUrl"),
                "",
            ),
            "usedFor": "playback via server-provided streamUrl (never constructed client-side)",
        }
        for client in usage["clients"]
        if any(entry["field"] == "Song.streamUrl" for entry in client.get("opaqueServerUrls", []))
    ]
    for route in routes:
        if route["path"] == "/song/stream/{}/{}/{}" and not route["usedBy"]:
            route["usedBy"] = stream_consumers
            if route["parityStatus"] == "documented-uncaptured":
                route["parityStatus"] = "client-used-uncaptured"

    routes.sort(key=lambda r: (r["path"], r["method"]))

    cross_surface = []
    for key, declared_names in sorted(declared_scenarios.items()):
        if key[1].startswith("/images/"):
            names = [name for name in declared_names if name in promoted_names]
            cross_surface.append(
                {
                    "method": key[0],
                    "path": key[1],
                    "servingSurface": "opensubsonic (OpenSubsonic ImageController, unversioned)",
                    "usedBy": ["melodee-player", "meloamp"],
                    "scenarios": names,
                    "note": "Native clients fetch artwork URLs with no auth headers; served by the OpenSubsonic surface.",
                }
            )

    summary: dict[str, int] = {}
    for route in routes:
        summary[route["parityStatus"]] = summary.get(route["parityStatus"], 0) + 1

    inventory = {
        "schemaVersion": SCHEMA_VERSION,
        "surface": "melodee-api-v1",
        "legacyBaselineCommit": manifest["legacyBaselineCommit"],
        "inputs": [
            "compatibility/melodee-api-v1/contracts/route-manifest.json",
            "compatibility/melodee-api-v1/contracts/openapi-v1.json",
            "compatibility/melodee-api-v1/contracts/native-client-usage.json",
            "compatibility/scenarios/*.json",
            "compatibility/melodee-api-v1/golden/*/*.metadata.json",
        ],
        "routeCount": len(routes),
        "summary": summary,
        "routes": routes,
        "crossSurfaceDependencies": cross_surface,
    }
    text = json.dumps(inventory, indent=2, ensure_ascii=False) + "\n"

    if args.check:
        if not args.output.exists() or args.output.read_text(encoding="utf-8") != text:
            print(f"DRIFT: {args.output.relative_to(ROOT)}", file=sys.stderr)
            raise SystemExit(1)
        print("route inventory up to date")
        return
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(text, encoding="utf-8")
    print(f"route inventory: {len(routes)} routes -> {args.output.relative_to(ROOT)}")
    for status, count in sorted(summary.items()):
        print(f"  {status}: {count}")


if __name__ == "__main__":
    main()
