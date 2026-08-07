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
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SURFACE_DIR = ROOT / "compatibility" / "melodee-api-v1"
SCENARIOS_DIR = ROOT / "compatibility" / "scenarios"
SCHEMA_VERSION = 1

UUID_RE = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
ROUTE_PARAM_RE = re.compile(r"\{[^}]*\}")


def normalize_path(path: str) -> str:
    """Normalize a concrete or templated path to the {} parameter form."""
    path = path.split("?", 1)[0]
    path = path.split("#", 1)[0]
    path = UUID_RE.sub("{}", path)
    path = ROUTE_PARAM_RE.sub("{}", path)
    # Literal trailing parameters in client-style paths (e.g. starred/{id}/true
    # captured with concrete values).
    path = re.sub(r"/(true|false|\d+)$", "/{}", path)
    return path


def scenario_route_map() -> dict[tuple[str, str], list[str]]:
    """Map (method, normalized path) -> scenario step names."""
    mapping: dict[tuple[str, str], list[str]] = {}
    for scenario_file in sorted(SCENARIOS_DIR.glob("*.json")):
        scenarios = json.loads(scenario_file.read_text(encoding="utf-8"))
        for scenario in scenarios:
            steps = scenario.get("flow") if isinstance(scenario.get("flow"), list) else [scenario]
            for index, step in enumerate(steps):
                name = f"{scenario['name']}/{step.get('name', f'step-{index + 1}')}" if steps is not [scenario] and isinstance(scenario.get("flow"), list) else str(scenario["name"])
                raw_path = str(step.get("path", ""))
                method = str(step.get("method", "GET")).upper()
                if "{{streamUrl}}" in raw_path:
                    normalized = "/song/stream/{}/{}/{}"
                elif "/song/stream/" in raw_path:
                    normalized = "/song/stream/{}/{}/{}"
                elif raw_path.startswith("/images/"):
                    normalized = normalize_path(raw_path)
                    mapping.setdefault((method, normalized), []).append(name)
                    continue
                else:
                    normalized = normalize_path(raw_path)
                mapping.setdefault((method, normalized), []).append(name)
    return mapping


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
    scenarios = scenario_route_map()

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
            scenario_names = scenarios.get(key, [])
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
    for key, names in sorted(scenarios.items()):
        if key[1].startswith("/images/"):
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
