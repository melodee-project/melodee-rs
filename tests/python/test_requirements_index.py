"""Guards against drift between the generated requirements index and prose that
quotes index-derived counts (AGENTS.md)."""

from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INDEX = ROOT / "design" / "requirements" / "REQUIREMENTS-INDEX.md"
AGENTS = ROOT / "AGENTS.md"

# Mirrors TERMINAL_STATUSES in scripts/audit_requirement_overrides.py.
TERMINAL_STATUSES = {"implemented", "tested", "deferred", "superseded"}

ROW_RE = re.compile(r"^\|\s`(?P<id>PRD-[A-Z0-9-]+)`\s\|\s(?P<title>[^|]+)\|\s(?P<status>[a-z-]+)\s\|")

AGENTS_COUNT_RE = re.compile(
    r"Non-terminal requirements remaining: \*\*(?P<total>\d+)\*\* \(status `in-progress`"
)


def status_counts() -> dict[str, int]:
    counts: dict[str, int] = {}
    for line in INDEX.read_text(encoding="utf-8").splitlines():
        match = ROW_RE.match(line)
        if match:
            status = match.group("status")
            counts[status] = counts.get(status, 0) + 1
    return counts


class RequirementsIndexTests(unittest.TestCase):
    def test_index_statuses_use_known_vocabulary(self) -> None:
        unknown = set(status_counts()) - TERMINAL_STATUSES - {"in-progress", "planned"}
        self.assertEqual(unknown, set(), f"unknown statuses in index: {sorted(unknown)}")

    def test_agents_non_terminal_count_matches_index(self) -> None:
        counts = status_counts()
        in_progress = counts.get("in-progress", 0)
        text = AGENTS.read_text(encoding="utf-8")
        match = AGENTS_COUNT_RE.search(text)
        self.assertIsNotNone(
            match,
            "AGENTS.md must state 'Non-terminal requirements remaining: **N** (status `in-progress`...)'",
        )
        assert match is not None  # narrow for type checkers
        self.assertEqual(
            int(match.group("total")),
            in_progress,
            "AGENTS.md non-terminal count drifted from the requirements index; update AGENTS.md",
        )
        self.assertGreater(in_progress, 0, "index has no in-progress rows; simplify AGENTS.md prose")


if __name__ == "__main__":
    unittest.main()
