#!/usr/bin/env python3
"""Audit requirement status overrides against the generated PRD inventory.

The normal audit validates structure and evidence-shape consistency while
allowing the current release-closure statuses.  ``--require-terminal`` is
available to the release gate and fails until every requirement has a truthful
terminal status.
"""

from __future__ import annotations

import argparse
import collections
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OVERRIDES = ROOT / "design" / "requirements" / "status-overrides.json"
sys.path.insert(0, str(ROOT / "scripts"))
from extract_requirements import extract  # noqa: E402


ALLOWED_STATUSES = {
    "planned",
    "in-progress",
    "captured",
    "implemented",
    "tested",
    "deferred",
    "superseded",
}
TERMINAL_STATUSES = {"implemented", "tested", "deferred", "superseded"}


class OverrideAuditError(ValueError):
    """Raised when the override registry cannot support a truthful audit."""


def audit(*, require_terminal: bool = False) -> dict[str, object]:
    """Validate override shape and return counts suitable for evidence."""

    try:
        overrides = json.loads(OVERRIDES.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise OverrideAuditError(f"unable to read override registry: {error}") from error
    if not isinstance(overrides, dict):
        raise OverrideAuditError("override registry must be a JSON object")
    if not isinstance(overrides.get("comment"), str) or not overrides["comment"].strip():
        raise OverrideAuditError("override registry must contain a non-empty comment")

    requirements = extract()
    requirement_ids = {str(item["id"]) for item in requirements}
    override_ids = set(overrides) - {"comment"}
    errors: list[str] = []
    missing = sorted(requirement_ids - override_ids)
    unknown = sorted(override_ids - requirement_ids)
    if missing:
        errors.append(f"missing overrides: {', '.join(missing)}")
    if unknown:
        errors.append(f"unknown override IDs: {', '.join(unknown)}")

    statuses: collections.Counter[str] = collections.Counter()
    nonterminal: list[str] = []
    for requirement_id in sorted(requirement_ids & override_ids):
        entry = overrides[requirement_id]
        if not isinstance(entry, dict):
            errors.append(f"{requirement_id}: override must be an object")
            continue
        status = entry.get("status")
        if not isinstance(status, str) or status not in ALLOWED_STATUSES:
            errors.append(f"{requirement_id}: invalid status {status!r}")
        else:
            statuses[status] += 1
            if status not in TERMINAL_STATUSES:
                nonterminal.append(requirement_id)
        for field in ("implementation", "tests"):
            value = entry.get(field, [])
            if not isinstance(value, list) or not all(
                isinstance(item, str) and item.strip() for item in value
            ):
                errors.append(f"{requirement_id}: {field} must be a list of non-empty strings")

    if require_terminal and nonterminal:
        errors.append(
            "non-terminal requirements: "
            + ", ".join(nonterminal)
            + " (use an accepted decision or complete evidence; do not waive them here)"
        )
    if errors:
        raise OverrideAuditError("; ".join(errors))
    return {
        "total": len(requirement_ids),
        "override_count": len(override_ids),
        "status_counts": dict(sorted(statuses.items())),
        "nonterminal": nonterminal,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--require-terminal",
        action="store_true",
        help="fail unless every requirement has a terminal truthful status",
    )
    args = parser.parse_args()
    try:
        result = audit(require_terminal=args.require_terminal)
    except OverrideAuditError as error:
        print(f"requirement override audit failed: {error}", file=sys.stderr)
        return 1
    counts = ", ".join(
        f"{status}={count}" for status, count in result["status_counts"].items()
    )
    print(
        f"requirement override audit passed: {result['total']} requirements, "
        f"{result['override_count']} overrides, {counts}; "
        f"nonterminal={len(result['nonterminal'])}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
