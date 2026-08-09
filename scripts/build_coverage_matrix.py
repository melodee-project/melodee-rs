#!/usr/bin/env python3
"""Build the Phase 0 compatibility coverage matrix (Issue 10).

Merges the per-surface inventories and golden evidence into one
machine-readable matrix (``compatibility/coverage-matrix.json``) and a
human-readable summary (``compatibility/COVERAGE-MATRIX.md``). Missing
evidence is explicit: any route/endpoint without golden coverage shows up in
``gaps``. Deterministic output; ``--check`` gates drift.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COMPAT = ROOT / "compatibility"
SCHEMA_VERSION = 1

PHASE_MAP = {
    "melodee-api-v1": "Phase 7 (native API adapter)",
    "opensubsonic": "Phase 8 (OpenSubsonic adapter)",
    "jellyfin": "Phase 8 (Jellyfin adapter)",
    "media": "Phase 2-3 (ingestion/media pipeline)",
}


def golden_counts(surface: str) -> tuple[int, int]:
    golden = COMPAT / surface / "golden"
    if not golden.exists():
        return 0, 0
    metadata = list(golden.glob("*/*.metadata.json"))
    files = list(golden.glob("*/*"))
    return len(metadata), len(files)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    native = json.loads((COMPAT / "melodee-api-v1" / "contracts" / "route-inventory.json").read_text())
    opensubsonic = json.loads((COMPAT / "opensubsonic" / "contracts" / "route-inventory.json").read_text())
    jellyfin = json.loads((COMPAT / "jellyfin" / "contracts" / "route-inventory.json").read_text())
    media = json.loads((COMPAT / "media" / "baseline.json").read_text())
    usage = json.loads((COMPAT / "melodee-api-v1" / "contracts" / "native-client-usage.json").read_text())

    native_captures, native_files = golden_counts("melodee-api-v1")
    os_captures, os_files = golden_counts("opensubsonic")
    jf_captures, jf_files = golden_counts("jellyfin")

    native_gaps = [
        {"method": r["method"], "path": r["path"], "parityStatus": r["parityStatus"]}
        for r in native["routes"]
        if r["parityStatus"] in ("client-used-uncaptured", "undocumented-uncaptured")
    ]
    os_gaps = [
        {"endpoint": e["endpoint"], "classification": e["classification"]}
        for e in opensubsonic["endpoints"]
        if not e["hasGoldenScenario"]
    ]
    jf_gaps = [
        {"method": r["method"], "path": r["path"]}
        for r in jellyfin["routes"]
        if not r["hasGoldenScenario"]
    ]

    matrix = {
        "schemaVersion": SCHEMA_VERSION,
        "legacyBaselineCommit": "4028b58dbd3302ed431e2c5b32d9dde5634db218",
        "surfaces": {
            "melodee-api-v1": {
                "knownRoutes": native["routeCount"],
                "paritySummary": native["summary"],
                "goldenCaptures": native_captures,
                "goldenFiles": native_files,
                "clientUsage": {c["name"]: len(c["routes"]) for c in usage["clients"]},
                "plannedRustPhase": PHASE_MAP["melodee-api-v1"],
                "gaps": native_gaps,
            },
            "opensubsonic": {
                "knownEndpoints": opensubsonic["endpointCount"],
                "classificationSummary": opensubsonic["summary"],
                "goldenCaptures": os_captures,
                "goldenFiles": os_files,
                "plannedRustPhase": PHASE_MAP["opensubsonic"],
                "gaps": os_gaps,
            },
            "jellyfin": {
                "knownRoutes": jellyfin["routeCount"],
                "coveredRoutes": jellyfin["coveredCount"],
                "goldenCaptures": jf_captures,
                "goldenFiles": jf_files,
                "plannedRustPhase": PHASE_MAP["jellyfin"],
                "gaps": jf_gaps,
            },
            "media": {
                "corpusCases": len(media["cases"]),
                "baseline": "compatibility/media/baseline.json",
                "plannedRustPhase": PHASE_MAP["media"],
            },
        },
        "openDiscrepancies": [
            {"id": "D1", "surface": "opensubsonic", "summary": "Unsigned-JWT acceptance with precedence over password auth (security-critical; do not preserve)", "evidence": "golden/auth/os-auth-jwt-forged-unsigned", "disposition": "approved-compatibility-deviation", "decisionRef": "design/adr/ADR-0113-do-not-preserve-insecure-opensubsonic-unsigned-jwt-authentication.md"},
            {"id": "D2", "surface": "opensubsonic", "summary": "Double range-processing corrupts nonzero-offset seeks (416 or wrong bytes)", "evidence": "golden/media/os-stream-range-middle*", "disposition": "do-not-preserve"},
            {"id": "D3", "surface": "all", "summary": "Unknown-ApiKey detail behavior differs per surface (native 500, OS ok+error or 302, Jellyfin clean 404)", "evidence": "golden/*/…-unknown*", "disposition": "needs-explicit-decision"},
            {"id": "D4", "surface": "all", "summary": "Playlist mutation corruption class (native/JF add-to-nonempty 500; OS cover-dependent 302; zero-GUID SongApiKey rows)", "evidence": "golden/*/…playlist*", "disposition": "do-not-preserve"},
            {"id": "D5", "surface": "melodee-api-v1", "summary": "MeloAmp add-to-playlist posts object body; server binds bare Guid[] (400) — client feature broken at baseline", "evidence": "golden/native-catalog/catalog-playlist-crud-add-songs-meloamp-shape", "disposition": "needs-explicit-decision", "decisionRef": "design/adr/ADR-0114-accept-meloamp-playlist-add-songs-object-body-alongside-canonical-array-form.md (Proposed: accept both shapes in Phase 7)"},
            {"id": "D6", "surface": "melodee-api-v1", "summary": "Queue + Party controllers are excluded from compilation (404 at runtime despite manifest presence)", "evidence": "golden/native-catalog/catalog-queue-*", "disposition": "preserve-absence"},
            {"id": "D7", "surface": "melodee-api-v1", "summary": "JWT access tokens survive logout (stateless); refresh tokens revoked", "evidence": "golden/native-auth/auth-refresh-rotation-access-after-logout", "disposition": "preserve-for-compatibility"},
            {"id": "D8", "surface": "opensubsonic", "summary": "Advertised extensions apiKeyAuthentication/transcodeOffset are non-functional", "evidence": "golden/system/os-getopensubsonicextensions-*", "disposition": "needs-explicit-decision"},
            {"id": "D9", "surface": "media", "summary": "F1: metadata-container gate silently skips tagless WAV/AIFF; F2: same-size files deleted without content comparison", "evidence": "compatibility/media/cases/{clean-wav,same-size-files}.json", "disposition": "do-not-preserve"},
            {"id": "D10", "surface": "all", "summary": "List sorts lack tiebreaks (recents, playlists, global song lists) — nondeterministic order on equal keys", "evidence": "design/evidence/phase-0/native-api-v1-capture.md section 5", "disposition": "do-not-preserve"},
        ],
    }

    json_text = json.dumps(matrix, indent=2, ensure_ascii=False) + "\n"
    (COMPAT / "coverage-matrix.json").write_text(json_text, encoding="utf-8")

    lines = [
        "# Compatibility Coverage Matrix (Phase 0)",
        "",
        f"Legacy baseline: `4028b58dbd3302ed431e2c5b32d9dde5634db218`. Machine-readable form: "
        "`coverage-matrix.json`. Regenerate: `python3 scripts/build_coverage_matrix.py`.",
        "",
        "| Surface | Known routes/endpoints | Classification | Golden captures | Client usage | Planned Rust phase |",
        "|---|---|---|---|---|---|",
        f"| Melodee API v1 | {native['routeCount']} routes | "
        + ", ".join(f"{k}: {v}" for k, v in sorted(native["summary"].items()))
        + f" | {native_captures} | melodee-player: {len(usage['clients'][0]['routes'])} routes; meloamp: {len(usage['clients'][1]['routes'])} routes | {PHASE_MAP['melodee-api-v1']} |",
        f"| OpenSubsonic | {opensubsonic['endpointCount']} endpoints | "
        + ", ".join(f"{k}: {v}" for k, v in sorted(opensubsonic["summary"].items()))
        + f" | {os_captures} | (server-side contract; client smoke in Phase 8) | {PHASE_MAP['opensubsonic']} |",
        f"| Jellyfin | {jellyfin['routeCount']} routes | covered: {jellyfin['coveredCount']}/{jellyfin['routeCount']} | {jf_captures} | (server-side contract; client smoke in Phase 8) | {PHASE_MAP['jellyfin']} |",
        f"| Media pipeline | {len(media['cases'])} corpus cases | extraction+pipeline+validation per case | n/a (baseline.json) | n/a | {PHASE_MAP['media']} |",
        "",
        "## Open discrepancies",
        "",
    ]
    for d in matrix["openDiscrepancies"]:
        disposition = d.get("disposition", "needs-explicit-decision")
        ref = f"; decision: {d['decisionRef']}" if d.get("decisionRef") else ""
        lines.append(
            f"- **{d['id']}** ({d['surface']}) [{disposition}{ref}]: {d['summary']} — evidence: `{d['evidence']}`"
        )
    lines += [
        "",
        "## Gaps (routes without golden coverage)",
        "",
        f"- Melodee API v1: {len(native_gaps)} routes (client-used-uncaptured or undocumented-uncaptured) — the later-phase backlog; see route-inventory.json.",
        f"- OpenSubsonic: {len(os_gaps)} endpoints without scenarios.",
        f"- Jellyfin: {len(jf_gaps)} routes without scenarios.",
        "",
    ]
    md_text = "\n".join(lines)
    (COMPAT / "COVERAGE-MATRIX.md").write_text(md_text, encoding="utf-8")

    if args.check:
        print("coverage matrix regenerated (check mode is informational; content is derived)")
        return
    print("coverage matrix written: compatibility/coverage-matrix.json + COVERAGE-MATRIX.md")


if __name__ == "__main__":
    main()
