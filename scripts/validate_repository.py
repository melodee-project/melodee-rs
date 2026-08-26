#!/usr/bin/env python3
from __future__ import annotations

import os
import re
import struct
import sys
import tomllib
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]

REQUIRED = [
    "README.md",
    "AGENTS.md",
    "PRD.md",
    "SPEC.md",
    "IMPLEMENTATION.md",
    "Cargo.toml",
    "rust-toolchain.toml",
    "deny.toml",
    ".github/workflows/ci.yml",
    "design/adr/README.md",
    "design/adr/TEMPLATE.md",
    "design/prompts/IMPLEMENT-WORK.md",
    "design/requirements/FEATURE-PARITY-MATRIX.md",
    "design/requirements/REQUIREMENTS-INDEX.md",
    "design/requirements/requirements.json",
    "design/ui/DESIGN-SYSTEM.md",
    "tests/README.md",
]

LINK_RE = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")
ADR_RE = re.compile(r"ADR-(\d{4})-[a-z0-9-]+\.md$")
STATUS_RE = re.compile(
    r"^- \*\*Status:\*\* (Proposed|Accepted|Superseded|Deprecated|Rejected)$", re.M
)
REQUIREMENT_RE = re.compile(r"^###\s+(PRD-[A-Z0-9-]+)", re.M)


def png_size(path: Path) -> tuple[int, int]:
    with path.open("rb") as stream:
        header = stream.read(24)
    if len(header) < 24 or header[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"not a PNG: {path}")
    return struct.unpack(">II", header[16:24])


# Vendored/generated trees are not repository documentation and are never
# committed; scanning them only reports upstream README issues.
SKIP_DIRS = {"node_modules", "dist", ".astro", ".git", "target", "__pycache__", ".venv"}

# The Starlight content tree uses route-style internal links (e.g.
# `./quick-start/`) that intentionally do not resolve to files on disk. Those
# links are validated by the docs toolchain instead (`pnpm check:links`,
# starlight-links-validator, and the Astro build in the docs CI workflow).
ROUTE_LINK_TREES = {ROOT / "docs" / "src" / "content"}


def iter_markdown():
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [name for name in dirnames if name not in SKIP_DIRS]
        current = Path(dirpath)
        if any(current == tree or tree in current.parents for tree in ROUTE_LINK_TREES):
            continue
        for filename in filenames:
            if filename.endswith(".md"):
                yield current / filename


def validate_links(errors: list[str]) -> None:
    for markdown in iter_markdown():
        text = markdown.read_text(encoding="utf-8")
        for raw in LINK_RE.findall(text):
            target = raw.split("#", 1)[0].strip().strip("<>")
            if not target or target.startswith(("http://", "https://", "mailto:")):
                continue
            target = unquote(target)
            candidate = (markdown.parent / target).resolve()
            try:
                candidate.relative_to(ROOT.resolve())
            except ValueError:
                errors.append(f"{markdown.relative_to(ROOT)}: link escapes repository: {raw}")
                continue
            if not candidate.exists():
                errors.append(f"{markdown.relative_to(ROOT)}: missing link target: {raw}")


def main() -> None:
    errors: list[str] = []

    for required in REQUIRED:
        if not (ROOT / required).exists():
            errors.append(f"missing required file: {required}")

    for toml_path in ["Cargo.toml", "rust-toolchain.toml", "deny.toml"]:
        try:
            with (ROOT / toml_path).open("rb") as stream:
                tomllib.load(stream)
        except Exception as error:  # noqa: BLE001 - validator reports all parse failures
            errors.append(f"invalid TOML {toml_path}: {error}")

    adrs = sorted((ROOT / "design" / "adr").glob("ADR-*.md"))
    seen_numbers: set[str] = set()
    for adr in adrs:
        match = ADR_RE.match(adr.name)
        if not match:
            errors.append(f"invalid ADR filename: {adr.name}")
            continue
        number = match.group(1)
        if number in seen_numbers:
            errors.append(f"duplicate ADR number: {number}")
        seen_numbers.add(number)
        content = adr.read_text(encoding="utf-8")
        if not STATUS_RE.search(content):
            errors.append(f"ADR missing valid bold Status line: {adr.name}")

    mockups = sorted((ROOT / "design" / "ui" / "reference").glob("*.png"))
    if len(mockups) != 8:
        errors.append(f"expected 8 canonical mockups, found {len(mockups)}")
    for mockup in mockups:
        try:
            width, height = png_size(mockup)
            if width < 1600 or height < 1200:
                errors.append(f"mockup too small: {mockup.name} is {width}x{height}")
        except ValueError as error:
            errors.append(str(error))

    prd_requirements = REQUIREMENT_RE.findall((ROOT / "PRD.md").read_text(encoding="utf-8"))
    if len(prd_requirements) < 200:
        errors.append(f"expected at least 200 PRD requirements, found {len(prd_requirements)}")
    if len(prd_requirements) != len(set(prd_requirements)):
        errors.append("duplicate PRD requirement IDs")

    validate_links(errors)

    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1)

    print(
        f"Repository validation passed ({len(adrs)} ADRs, {len(mockups)} mockups, "
        f"{len(prd_requirements)} requirements)."
    )


if __name__ == "__main__":
    main()
