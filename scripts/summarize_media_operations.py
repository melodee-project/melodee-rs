#!/usr/bin/env python3
"""Summarize fixed-label media-operation TRACE events from a JSON log.

Input is streamed with explicit size limits. Output contains no source paths,
work-item IDs, or raw log lines. Missing, malformed, or truncated telemetry is
reported as incomplete; it must not be interpreted as zero work.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import TextIO
from uuid import UUID

LABELS = ("acquisition_tree_hash", "acquisition_copy", "media_file_hash", "media_buffer_hash", "media_bounded_read",
          "media_copy", "validation_hash", "publication_copy", "publication_hash",
          "ffprobe", "ffmpeg", "fpcalc", "tool_version", "other_tool")
MAX_LINE = 1024 * 1024
MAX_BYTES = 128 * 1024 * 1024
MAX_EVENTS = 2_000_000
MAX_PROCESSES = 32
MAX_U64 = (1 << 64) - 1


def summarize(stream: TextIO) -> dict:
    totals = {label: {"operations": 0, "succeeded": 0, "read_bytes": 0,
                      "written_bytes": 0, "incomplete_byte_counts": 0}
              for label in LABELS}
    seen = set()
    consumed = events = malformed = 0
    limited = False
    while line := stream.readline(MAX_LINE + 1):
        consumed += len(line.encode("utf-8"))
        if consumed > MAX_BYTES or len(line) > MAX_LINE or not line.endswith("\n"):
            limited = True
            break
        try:
            entry = json.loads(line)
        except (ValueError, TypeError):
            malformed += 1
            continue
        if not isinstance(entry, dict) or entry.get("target") != "melodee::media_operations":
            continue
        fields = entry.get("fields")
        if not isinstance(fields, dict) or fields.get("message") != "media operation completed":
            malformed += 1
            continue
        label = fields.get("operation")
        success, complete = fields.get("success"), fields.get("complete_bytes")
        read, written, elapsed = (fields.get("read_bytes"), fields.get("written_bytes"),
                                  fields.get("elapsed_micros"))
        token, pid = fields.get("process_token"), fields.get("process_id")
        try:
            if (label not in totals or type(success) is not bool or type(complete) is not bool
                    or any(type(value) is not int or not 0 <= value <= MAX_U64 for value in (read, written, elapsed))
                    or type(pid) is not int or pid < 1 or not isinstance(token, str)):
                raise ValueError("invalid operation event")
            UUID(token)
        except (ValueError, TypeError, AttributeError):
            malformed += 1
            continue
        seen.add((pid, token))
        events += 1
        if events > MAX_EVENTS or len(seen) > MAX_PROCESSES:
            limited = True
            break
        item = totals[label]
        for key, amount in (("operations", 1), ("succeeded", int(success)),
                            ("read_bytes", read), ("written_bytes", written),
                            ("incomplete_byte_counts", int(not complete))):
            if item[key] > MAX_U64 - amount:
                limited = True
            item[key] = min(MAX_U64, item[key] + amount)
    return {"complete": bool(events) and not limited and malformed == 0,
            "byte_counts_complete": bool(events) and not limited and malformed == 0
                                    and not any(item["incomplete_byte_counts"] for item in totals.values()),
            "events": events, "processes": len(seen), "malformed_lines": malformed,
            "input_limited_or_truncated": limited, "operations": totals,
            "limitations": ["counts instrumented operations only; not physical I/O",
                            "media_buffer_hash read_bytes are memory bytes hashed, not additional file reads",
                            "external subprocess file I/O is not counted",
                            "failed operations may report lower-bound bytes",
                            "events span the whole captured server process, not an isolated stage",
                            "TRACE logging changes runtime overhead"]}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path, help="structured JSON server log (read only)")
    args = parser.parse_args()
    with args.log.open(encoding="utf-8", errors="replace") as source:
        result = summarize(source)
    print(json.dumps(result, sort_keys=True))
    return 0 if result["complete"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
