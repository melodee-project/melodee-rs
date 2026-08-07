#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ADR_DIR = ROOT / "design" / "adr"
TEMPLATE = ADR_DIR / "TEMPLATE.md"


def slugify(value: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")
    if not slug:
        raise ValueError("title does not contain usable characters")
    return slug


def next_number() -> int:
    numbers = []
    for path in ADR_DIR.glob("ADR-[0-9][0-9][0-9][0-9]-*.md"):
        match = re.match(r"ADR-(\d{4})-", path.name)
        if match:
            numbers.append(int(match.group(1)))
    return max(numbers, default=99) + 1


def main() -> None:
    parser = argparse.ArgumentParser(description="Create the next Melodee ADR")
    parser.add_argument("title")
    args = parser.parse_args()

    number = next_number()
    identifier = f"ADR-{number:04d}"
    path = ADR_DIR / f"{identifier}-{slugify(args.title)}.md"
    if path.exists():
        raise SystemExit(f"Refusing to overwrite {path}")

    content = TEMPLATE.read_text(encoding="utf-8")
    content = content.replace("ADR-XXXX: Short title", f"{identifier}: {args.title}")
    content = content.replace("YYYY-MM-DD", date.today().isoformat(), 1)
    path.write_text(content, encoding="utf-8")
    print(path.relative_to(ROOT))


if __name__ == "__main__":
    main()
