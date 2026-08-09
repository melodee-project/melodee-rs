#!/usr/bin/env python3
"""Build the OpenSubsonic endpoint inventory (Issue 6).

Groups the static route manifest into canonical endpoints (plain and `.view`
forms together) and records, per endpoint: methods, controller, auth model,
request encodings, response formats, implementation classification
(supported / conditional / gone / not-implemented / excluded-from-build),
feature or role prerequisites, expected negative status, and golden scenario
coverage. Deterministic output; ``--check`` gates drift.

OpenSubsonic auth and format conventions are uniform at the controller base
(query + form POST auth params; JSON/XML/JSONP via f=), so they are recorded
once at the inventory level rather than repeated per endpoint; deviations are
recorded per endpoint in ``notes``.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SURFACE_DIR = ROOT / "compatibility" / "opensubsonic"
SCENARIOS_DIR = ROOT / "compatibility" / "scenarios"
SCHEMA_VERSION = 1

# Prerequisite/notes knowledge captured from runtime evidence (Issue 6 golden).
PREREQUISITES = {
    "download": "system.isDownloadingEnabled (seeded true)",
    "jukeboxControl": "jukebox.enabled AND jukebox.backendType non-empty; else 410",
    "startScan": "no admin requirement at baseline (any authenticated user)",
    "createInternetRadioStation": "admin",
    "updateInternetRadioStation": "admin",
    "deleteInternetRadioStation": "admin",
    "createShare": "user CanShare capability",
}

PODCAST_ENDPOINTS = {
    "getPodcasts", "getNewestPodcasts", "refreshPodcasts", "createPodcastChannel",
    "deletePodcastChannel", "deletePodcastEpisode", "downloadPodcastEpisode",
    "streamPodcastEpisode",
}

EXTENSION_ENDPOINTS = {"searchForArtist", "searchForArtistImage", "searchForAlbumImage"}


def endpoint_name(path: str) -> str | None:
    match = re.match(r"^/rest/([A-Za-z0-9]+?)(?:\.view)?$", path)
    return match.group(1) if match else None


def scenario_endpoints() -> dict[str, list[str]]:
    mapping: dict[str, list[str]] = {}
    for scenario_file in sorted(SCENARIOS_DIR.glob("opensubsonic-*.json")):
        for scenario in json.loads(scenario_file.read_text(encoding="utf-8")):
            steps = scenario.get("flow") if isinstance(scenario.get("flow"), list) else [scenario]
            for index, step in enumerate(steps):
                path = str(step.get("path", "")).split("?", 1)[0]
                name = endpoint_name(path)
                if name:
                    step_name = f"{scenario['name']}/{step.get('name', f'step-{index + 1}')}" if isinstance(scenario.get("flow"), list) else str(scenario["name"])
                    mapping.setdefault(name, []).append(step_name)
    return mapping


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=SURFACE_DIR / "contracts" / "route-inventory.json")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    manifest = json.loads((SURFACE_DIR / "contracts" / "route-manifest.json").read_text(encoding="utf-8"))
    scenarios = scenario_endpoints()

    endpoints: dict[str, dict[str, object]] = {}
    for route in manifest["routes"]:
        name = endpoint_name(route["path"])
        if not name:
            # Root /ping alias and other non-/rest forms.
            if route["path"] == "/ping":
                name = "ping"
            else:
                continue
        entry = endpoints.setdefault(
            name,
            {
                "endpoint": name,
                "routes": [],
                "controller": route["controller"],
                "implementation": route["implementation"],
                "intentionalStatuses": route["intentionalStatuses"],
                "methods": set(),
                "scenarios": [],
            },
        )
        entry["routes"].append(route["path"])
        entry["methods"].update(route["methods"])
        if route["implementation"] in ("gone", "not-implemented") and entry["implementation"] == "implemented":
            entry["implementation"] = route["implementation"]

    rows = []
    summary: dict[str, int] = {}
    for name, entry in sorted(endpoints.items()):
        scenario_names = sorted(set(scenarios.get(name, [])))
        implementation = str(entry["implementation"])
        if name in PODCAST_ENDPOINTS:
            prerequisite = "podcast.enabled (seeded true); podcast role; auth failures return HTTP 403 (unique to this controller); disabled -> HTTP 400 error 0"
            classification = "conditional"
        elif name == "jukeboxControl":
            prerequisite = PREREQUISITES[name]
            classification = "conditional"
        elif implementation == "gone":
            prerequisite = None
            classification = "gone"
        elif implementation == "not-implemented":
            prerequisite = None
            classification = "not-implemented"
        elif implementation == "excluded-from-build":
            prerequisite = None
            classification = "excluded-from-build"
        else:
            prerequisite = PREREQUISITES.get(name)
            classification = "supported" if not prerequisite else "conditional"
        expected_status = None
        if classification == "gone":
            expected_status = 410
        elif classification == "not-implemented":
            expected_status = 501
        rows.append(
            {
                "endpoint": name,
                "routes": sorted(set(entry["routes"])),
                "methods": sorted(entry["methods"]),
                "controller": entry["controller"],
                "classification": classification,
                "prerequisite": prerequisite,
                "expectedUnsupportedStatus": expected_status,
                "intentionalStatuses": entry["intentionalStatuses"],
                "scenarios": scenario_names,
                "hasGoldenScenario": bool(scenario_names),
                "isMelodeeExtension": name in EXTENSION_ENDPOINTS,
            }
        )
        summary[classification] = summary.get(classification, 0) + 1

    inventory = {
        "schemaVersion": SCHEMA_VERSION,
        "surface": "opensubsonic",
        "legacyBaselineCommit": manifest["legacyBaselineCommit"],
        "authModel": {
            "transports": ["query string", "form POST (all endpoints)"],
            "forms": [
                "u + t + s (token = md5(opensubsonicSecret+salt), case-insensitive hex)",
                "u + p (plaintext or enc: hex)",
                "jwt param (NO signature verification; requires u + any p/t to reach the branch; takes precedence)",
                "apiKey param: advertised (apiKeyAuthentication[1]) but NOT honored",
            ],
            "bypass": "no credentials required when request is local, path contains /images/, or request host == system.baseUrl; no user synthesized (BlankUserInfo)",
            "failure": "HTTP 200 + subsonic-response error 40 (podcast controller: HTTP 403)",
        },
        "formats": {
            "default": "xml",
            "json": "f=json",
            "jsonp": "f=jsonp&callback=... (without callback falls back to XML)",
            "envelope": "subsonic-response {status, version, type, serverVersion, openSubsonic:true}; XML type is hardcoded lowercase 'melodee'",
        },
        "endpointCount": len(rows),
        "summary": summary,
        "endpoints": rows,
    }
    text = json.dumps(inventory, indent=2, ensure_ascii=False) + "\n"
    if args.check:
        if not args.output.exists() or args.output.read_text(encoding="utf-8") != text:
            print(f"DRIFT: {args.output.relative_to(ROOT)}", file=sys.stderr)
            raise SystemExit(1)
        print("opensubsonic inventory up to date")
        return
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(text, encoding="utf-8")
    print(f"opensubsonic inventory: {len(rows)} endpoints -> {args.output.relative_to(ROOT)}")
    for status, count in sorted(summary.items()):
        print(f"  {status}: {count}")


if __name__ == "__main__":
    main()
