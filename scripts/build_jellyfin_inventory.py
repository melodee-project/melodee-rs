#!/usr/bin/env python3
"""Build the Jellyfin compatibility route inventory (Issue 7).

Per route from the static manifest: controller/action, methods, routing
behavior (middleware rewrite rules), auth requirement, and golden scenario
coverage. The inventory also records the middleware detection rules and the
explicit unsupported-scope boundary. Deterministic output; ``--check`` gates
drift.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SURFACE_DIR = ROOT / "compatibility" / "jellyfin"
SCENARIOS_DIR = ROOT / "compatibility" / "scenarios"
SCHEMA_VERSION = 1

UUID_RE = re.compile(r"[0-9a-fA-F]{32}|[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
ROUTE_PARAM_RE = re.compile(r"\{[^}]*\}")


def normalize_path(path: str) -> str:
    path = path.split("?", 1)[0]
    path = UUID_RE.sub("{}", path)
    path = ROUTE_PARAM_RE.sub("{}", path)
    return path


def scenario_routes() -> list[tuple[str, str, str]]:
    """(method, normalized path, step name) tuples; template matching happens
    against manifest paths, not string equality, so literal route values
    (e.g. Images/Primary, stream.mp3) match their parameterized templates."""
    rows: list[tuple[str, str, str]] = []
    for scenario_file in sorted(SCENARIOS_DIR.glob("jellyfin-*.json")):
        for scenario in json.loads(scenario_file.read_text(encoding="utf-8")):
            steps = scenario.get("flow") if isinstance(scenario.get("flow"), list) else [scenario]
            for index, step in enumerate(steps):
                raw = str(step.get("path", ""))
                method = str(step.get("method", "GET")).upper()
                step_name = f"{scenario['name']}/{step.get('name', f'step-{index + 1}')}" if isinstance(scenario.get("flow"), list) else str(scenario["name"])
                rows.append((method, raw.split("?", 1)[0], step_name))
    return rows


def path_matches(template: str, concrete: str) -> bool:
    """Segment-wise match: template '{}' params accept any non-empty segment;
    literal segments must match exactly (after stripping /api/jf)."""
    t_parts = [p for p in template.split("/") if p]
    c_parts = [p for p in concrete.split("/") if p]
    if len(t_parts) != len(c_parts):
        return False
    for t, c in zip(t_parts, c_parts):
        if t == "{}":
            if not c:
                return False
        elif "{}" in t:
            # Partial parameter (e.g. "stream.{extension}" -> "stream.mp3").
            prefix = t.split("{", 1)[0]
            if not c.startswith(prefix):
                return False
        elif t.lower() != c.lower():
            return False
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=SURFACE_DIR / "contracts" / "route-inventory.json")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    manifest = json.loads((SURFACE_DIR / "contracts" / "route-manifest.json").read_text(encoding="utf-8"))
    scenario_rows = scenario_routes()

    routes = []
    covered = 0
    for route in manifest["routes"]:
        for method in route["methods"]:
            normalized = normalize_path(route["path"])
            # Manifest paths carry the /api/jf prefix; scenario paths use the
            # client-visible root form.
            client_form = normalized[len("/api/jf"):] if normalized.startswith("/api/jf") else normalized
            scenario_names = sorted(
                {
                    step_name
                    for row_method, row_path, step_name in scenario_rows
                    if row_method == method
                    and (path_matches(normalized, row_path) or path_matches(client_form, row_path))
                }
            )
            if scenario_names:
                covered += 1
            routes.append(
                {
                    "method": method,
                    "path": normalized,
                    "clientVisiblePath": client_form,
                    "controller": route["controller"],
                    "action": route["action"],
                    "implementation": route["implementation"],
                    "scenarios": scenario_names,
                    "hasGoldenScenario": bool(scenario_names),
                }
            )

    routes.sort(key=lambda r: (r["path"], r["method"]))
    inventory = {
        "schemaVersion": SCHEMA_VERSION,
        "surface": "jellyfin",
        "legacyBaselineCommit": manifest["legacyBaselineCommit"],
        "routing": {
            "middleware": "JellyfinRoutingMiddleware rewrites detected requests to /api/jf{path} before endpoint selection; excluded prefixes /api/, /rest/, /song/ pass through",
            "detection": [
                "allowlisted pre-auth paths (rewritten regardless of headers): /System/Info/Public, /System/Ping, /Users/AuthenticateByName, /Users/Public",
                "Authorization or X-Emby-Authorization header starting with MediaBrowser",
                "X-MediaBrowser-Token or X-Emby-Token header present",
                "api_key query param AND path under a Jellyfin prefix (/Audio/, /Items/, /Users/, /UserViews, /System/, /Sessions/, /Artists/, /Playlists/, /Images/)",
            ],
            "disabled": "jellyfin.enabled=false -> explicit /api/jf/* gets a hard middleware 404; detected root-form requests pass through unmodified (Blazor app or natural 404)",
            "root": "GET/HEAD / only rewritten when a Jellyfin marker is present (-> /api/jf, 200 empty); otherwise Blazor app",
        },
        "authModel": {
            "issuance": "POST /Users/AuthenticateByName {Username, Pw} (no Password field) -> {User, SessionInfo, AccessToken(64-hex), ServerId}; token = 32 random bytes hex; HMAC-SHA256(pepper, salt+token) stored, prefix-cleartext lookup; expiry jellyfin.token.expiresAfterHours (default 168h); max-active 10 (oldest soft-revoked)",
            "transports": ["Authorization: MediaBrowser ... Token=", "X-Emby-Authorization", "X-MediaBrowser-Token", "X-Emby-Token", "api_key query"],
            "failure": "401 JellyfinProblemDetails; revoked/expired tokens are indistinguishable from invalid",
            "logout": "POST /Sessions/Logout revokes the presented token (204; idempotent); tokens are stateful — revoked tokens die immediately (unlike stateless native JWTs)",
        },
        "routeCount": len(routes),
        "coveredCount": covered,
        "unsupportedScope": "No controllers exist for video/TV/books/LiveTV/plugins/admin; detected requests to such paths get ASP.NET 404 (empty body); non-music includeItemTypes are silently dropped from Items queries",
        "routes": routes,
    }
    text = json.dumps(inventory, indent=2, ensure_ascii=False) + "\n"
    if args.check:
        if not args.output.exists() or args.output.read_text(encoding="utf-8") != text:
            print(f"DRIFT: {args.output.relative_to(ROOT)}", file=sys.stderr)
            raise SystemExit(1)
        print("jellyfin inventory up to date")
        return
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(text, encoding="utf-8")
    print(f"jellyfin inventory: {len(routes)} routes ({covered} with scenarios) -> {args.output.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
