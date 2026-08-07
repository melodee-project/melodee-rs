#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PRD = ROOT / "PRD.md"
JSON_OUT = ROOT / "design" / "requirements" / "requirements.json"
MARKDOWN_OUT = ROOT / "design" / "requirements" / "REQUIREMENTS-INDEX.md"
OVERRIDES = ROOT / "design" / "requirements" / "status-overrides.json"

PHASE_BY_PREFIX = {
    "PRD-PRINCIPLE": "all",
    "PRD-SCALE": "0,6",
    "PRD-ARCH": "1",
    "PRD-INGEST": "2",
    "PRD-MEDIA": "3",
    "PRD-FP": "3,5",
    "PRD-ARTIST": "4",
    "PRD-RESOLVE": "4",
    "PRD-CREDIT": "4",
    "PRD-CLASS": "5",
    "PRD-POLICY": "5",
    "PRD-STORAGE": "2,3,11",
    "PRD-CATALOG": "4",
    "PRD-SEARCH": "6",
    "PRD-API-M1": "7",
    "PRD-STREAM": "7,8",
    "PRD-API-OS": "8",
    "PRD-API-JF": "8",
    "PRD-API-ADMIN": "10",
    "PRD-AUTH": "7,8,9",
    "PRD-USER": "9",
    "PRD-PLAYLIST": "9",
    "PRD-REQUEST": "9",
    "PRD-SHARE": "9",
    "PRD-POD": "9",
    "PRD-DISCOVERY": "9",
    "PRD-PARTY": "9",
    "PRD-DEVICE": "9",
    "PRD-SCRIPT": "9",
    "PRD-EXT": "4,9",
    "PRD-UI": "10",
    "PRD-UI-REF": "10",
    "PRD-THEME": "10",
    "PRD-I18N": "10",
    "PRD-WEB": "9,10",
    "PRD-SETUP": "9",
    "PRD-CONFIG": "1,9",
    "PRD-WORK": "1,2,9",
    "PRD-OBS": "1,2",
    "PRD-PERF": "0,all",
    "PRD-MIG": "11",
    "PRD-TEST": "0,all",
    "PRD-SEC": "all",
    "PRD-DEPLOY": "1,11",
}

HEADING_RE = re.compile(r"^###\s+(PRD-[A-Z0-9-]+)\s*(?:—|-)?\s*(.*)$")


def determine_phase(requirement_id: str) -> str:
    for prefix in sorted(PHASE_BY_PREFIX, key=len, reverse=True):
        if requirement_id.startswith(prefix):
            return PHASE_BY_PREFIX[prefix]
    return "unmapped"


def load_overrides() -> dict[str, dict[str, object]]:
    """Status/evidence overrides merged into the generated index. Kept in a
    separate file because the index itself is generated and must never be
    edited manually."""
    if not OVERRIDES.exists():
        return {}
    return json.loads(OVERRIDES.read_text(encoding="utf-8"))


def extract() -> list[dict[str, object]]:
    overrides = load_overrides()
    requirements = []
    for line_number, line in enumerate(PRD.read_text(encoding="utf-8").splitlines(), start=1):
        match = HEADING_RE.match(line)
        if not match:
            continue
        requirement_id, title = match.groups()
        override = overrides.get(requirement_id, {})
        requirements.append(
            {
                "id": requirement_id,
                "title": title.strip() or requirement_id,
                "phase": determine_phase(requirement_id),
                "source": "PRD.md",
                "line": line_number,
                "status": override.get("status", "planned"),
                "implementation": override.get("implementation", []),
                "tests": override.get("tests", []),
            }
        )

    ids = [item["id"] for item in requirements]
    duplicates = sorted({item for item in ids if ids.count(item) > 1})
    if duplicates:
        raise ValueError(f"duplicate requirement IDs: {duplicates}")
    return requirements


def render_markdown(requirements: list[dict[str, object]]) -> str:
    lines = [
        "# Requirements Index",
        "",
        "Generated from `PRD.md` by `scripts/extract_requirements.py`. Do not edit manually.",
        "",
        f"Total requirements: **{len(requirements)}**",
        "",
        "| Requirement | Title | Planned phase | Status | Implementation evidence | Test evidence |",
        "|---|---|---:|---|---|---|",
    ]
    for item in requirements:
        implementation = "; ".join(str(x) for x in item["implementation"]) or "—"
        tests = "; ".join(str(x) for x in item["tests"]) or "—"
        lines.append(
            f"| `{item['id']}` | {item['title']} | {item['phase']} | {item['status']} | {implementation} | {tests} |"
        )
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    requirements = extract()
    expected_json = json.dumps(requirements, indent=2, ensure_ascii=False) + "\n"
    expected_markdown = render_markdown(requirements)

    if args.check:
        failures = []
        if not JSON_OUT.exists() or JSON_OUT.read_text(encoding="utf-8") != expected_json:
            failures.append(str(JSON_OUT.relative_to(ROOT)))
        if not MARKDOWN_OUT.exists() or MARKDOWN_OUT.read_text(encoding="utf-8") != expected_markdown:
            failures.append(str(MARKDOWN_OUT.relative_to(ROOT)))
        if failures:
            raise SystemExit(
                "Generated requirement files are stale: " + ", ".join(failures) +
                ". Run python3 scripts/extract_requirements.py"
            )
        print(f"Requirements index is current ({len(requirements)} requirements).")
        return

    JSON_OUT.parent.mkdir(parents=True, exist_ok=True)
    JSON_OUT.write_text(expected_json, encoding="utf-8")
    MARKDOWN_OUT.write_text(expected_markdown, encoding="utf-8")
    print(f"Wrote {len(requirements)} requirements.")


if __name__ == "__main__":
    main()
