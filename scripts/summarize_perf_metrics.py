#!/usr/bin/env python3
"""Summarize Melodee's opt-in JSON performance snapshots from one run.

Snapshots are cumulative and their buckets are disjoint. The report compares
the first and last snapshot for each process token and metric, so activity
outside that interval is not included. Quantiles are bucket upper bounds, not
exact latency percentiles. An overflow quantile has no finite upper bound.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any, TextIO
from uuid import UUID

BUCKETS = (
    ("bucket_1ms", 1), ("bucket_5ms", 5), ("bucket_25ms", 25),
    ("bucket_100ms", 100), ("bucket_500ms", 500), ("bucket_2500ms", 2500),
    ("bucket_10000ms", 10000), ("bucket_60000ms", 60000), ("bucket_overflow", None),
)
MAX_SERIES = 512
MAX_LINE_CHARS = 1024 * 1024
METRIC_NAME = re.compile(r"[a-z][a-z0-9_]{0,63}\Z")


def snapshot(fields: dict[str, Any]) -> tuple[int, int, tuple[int, ...]]:
    values = [fields.get("count"), fields.get("sum_micros")]
    values.extend(fields.get(name) for name, _ in BUCKETS)
    if any(type(value) is not int or value < 0 for value in values):
        raise ValueError("snapshot counters must be nonnegative integers")
    count, total, *buckets = values
    if sum(buckets) != count:
        raise ValueError("snapshot bucket totals differ from the observation count")
    return count, total, tuple(buckets)


def percentile_upper_ms(buckets: tuple[int, ...], percentile: int) -> int | None:
    total = sum(buckets)
    if not total:
        return None
    threshold = (total * percentile + 99) // 100
    cumulative = 0
    for count, (_, upper) in zip(buckets, BUCKETS, strict=True):
        cumulative += count
        if cumulative >= threshold:
            return upper
    raise ValueError("invalid histogram")


def summarize(stream: TextIO) -> list[dict[str, Any]]:
    series: dict[tuple[str, str], dict[str, Any]] = {}
    while line := stream.readline(MAX_LINE_CHARS + 1):
        if len(line) > MAX_LINE_CHARS or not line.endswith("\n"):
            raise ValueError("input line exceeds limit or is incomplete")
        entry = json.loads(line)
        if not isinstance(entry, dict) or entry.get("target") != "melodee::performance":
            continue
        fields = entry.get("fields")
        if not isinstance(fields, dict):
            raise ValueError("performance event has no fields")
        token, metric = fields.get("process_token"), fields.get("metric")
        if not isinstance(token, str) or not isinstance(metric, str) or not METRIC_NAME.fullmatch(metric):
            raise ValueError("invalid process token or metric")
        UUID(token)
        value = snapshot(fields)
        key = (token, metric)
        if key not in series:
            if len(series) >= MAX_SERIES:
                raise ValueError("too many metric series")
            series[key] = {"first": value, "last": value, "samples": 1,
                           "first_timestamp": entry.get("timestamp"), "last_timestamp": entry.get("timestamp")}
        else:
            previous = series[key]["last"]
            if value[0] < previous[0] or value[1] < previous[1] or any(a < b for a, b in zip(value[2], previous[2], strict=True)):
                raise ValueError("cumulative snapshot counter decreased")
            series[key]["last"] = value
            series[key]["samples"] += 1
            series[key]["last_timestamp"] = entry.get("timestamp")

    output = []
    for (token, metric), item in sorted(series.items()):
        first, last = item["first"], item["last"]
        counts = tuple(end - start for start, end in zip(first[2], last[2], strict=True))
        output.append({
            "process_token": token, "metric": metric, "snapshots": item["samples"],
            "start_timestamp": item["first_timestamp"], "end_timestamp": item["last_timestamp"],
            "observations": last[0] - first[0], "sum_micros": last[1] - first[1],
            "bucket_counts": dict(zip((name for name, _ in BUCKETS), counts, strict=True)),
            "p50_upper_ms": percentile_upper_ms(counts, 50),
            "p95_upper_ms": percentile_upper_ms(counts, 95),
            "p99_upper_ms": percentile_upper_ms(counts, 99),
        })
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path, help="JSON log from one benchmark run")
    args = parser.parse_args()
    try:
        with args.log.open(encoding="utf-8") as stream:
            rows = summarize(stream)
        if not rows:
            raise ValueError("no performance snapshots found; check the logging filter")
        print(json.dumps({"quantile_kind": "bucket upper bound in milliseconds; null means no samples or overflow", "series": rows}, indent=2))
        return 0
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Cannot summarize performance metrics: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
