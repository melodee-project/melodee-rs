#!/usr/bin/env python3
"""Measure bounded missing-path recovery on an owned representative fixture.

Run inside perf_representative.py with a release server and build record.
This inserts synthetic work items into the disposable database; it never
deletes data. The enclosing fixture owner is responsible for disposal.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from build_perf_server import file_digest
from perf_genre_scheduler import query, start_server
from perf_ingress_workload import host_info, server_resources, stop_server, temporary_storage_info
from run_db_competition_matrix import verify_provenance
from run_perf_mutation_benchmarks import validate_fixture
import test_database

ROOT = Path(__file__).resolve().parents[1]
MAX_ITEMS = 35_000
MAX_TIMEOUT = 1_800
POLL_SECONDS = 0.5

SHAPE_SQL = """SELECT json_build_object('database',current_database(),
 'artists',(SELECT count(*) FROM artists), 'releases',(SELECT count(*) FROM releases),
 'tracks',(SELECT count(*) FROM tracks), 'work',(SELECT count(*) FROM release_work_items))"""
SAMPLE_SQL = """SELECT json_build_object(
 'total',(SELECT count(*) FROM release_work_items WHERE source_path LIKE 'benchmark-missing-%'),
 'missing',(SELECT count(*) FROM release_work_items WHERE source_path LIKE 'benchmark-missing-%' AND state_reason='source-missing'),
 'scan',(SELECT json_build_object('phase',phase,'revalidation',missing_revalidation,
       'retry',missing_retry,'reset',recovery_reset,'has_cursor',clear_after IS NOT NULL)
       FROM ingestion_discovery_scan WHERE scope_key='default'),
 'lock_waiters',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
       AND pid<>pg_backend_pid() AND wait_event_type='Lock'))"""
RETRY_PLAN_SQL = """EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON)
 SELECT EXISTS(SELECT 1 FROM release_work_items WHERE source_library_id IS NOT DISTINCT FROM NULL
   AND discovery_missing_retry_generation='00000000-0000-0000-0000-000000000001'::uuid
   AND inbound_active AND lease_owner IS NULL AND state IN ('discovered','settling','ready'))"""


def validate_options(items: int, timeout: int) -> None:
    if not 1 <= items <= MAX_ITEMS or not 1 <= timeout <= MAX_TIMEOUT:
        raise ValueError("items or timeout outside the owned benchmark limits")


def seed_sql(items: int) -> str:
    validate_options(items, 1)
    return f"""WITH inserted AS (
      INSERT INTO release_work_items (id,api_key,source_path,current_path,state,correlation_id,
          discovered_at,updated_at,created_at)
      SELECT gen_random_uuid(),gen_random_uuid(),'benchmark-missing-' || n,
          'benchmark-missing-' || n,'discovered',gen_random_uuid(),clock_timestamp(),
          clock_timestamp(),clock_timestamp() FROM generate_series(1,{items}) n RETURNING id)
      SELECT json_build_object('inserted',count(*)) FROM inserted"""


def run(items: int, timeout: int, server: Path, provenance_path: Path,
        env: dict[str, str], temp_parent: Path | None) -> int:
    validate_options(items, timeout)
    validate_fixture(env)
    name = env["MELODEE_PERF_OWNED_DATABASE"]
    url = env["MELODEE__DATABASE__URL"]
    admin = env.get("MELODEE_TEST_DATABASE_URL", "")
    if not admin or test_database.database_url(admin, name) != url:
        raise ValueError("fixture and administrator database topology differ")
    if test_database.sql(url, "SELECT current_database()") != name:
        raise ValueError("fixture database identity mismatch")
    server = server.resolve(strict=True)
    if not server.is_file() or provenance_path.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("invalid server or build record")
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    verify_provenance(ROOT, server, provenance)
    shape = query(url, SHAPE_SQL)
    if shape != {"database": name, "artists": 35000, "releases": 50000,
                 "tracks": 600000, "work": 0}:
        raise ValueError("expected an untouched 35K/50K/600K fixture")
    if query(url, seed_sql(items)) != {"inserted": items}:
        raise RuntimeError("work-item seed count mismatch")
    # The representative fixture is invocation-owned; update planner stats
    # after inserting the synthetic workload before recording its actual plan.
    test_database.sql(url, "ANALYZE release_work_items")
    retry_plan = json.loads(test_database.sql(url, RETRY_PLAN_SQL))
    with tempfile.TemporaryDirectory(prefix="melodee-perf-missing-", dir=temp_parent) as temporary:
        root = Path(temporary)
        for folder in ("inbound", "staging", "work", "storage", "assets"):
            (root / folder).mkdir()
        storage = temporary_storage_info(root)
        started = time.monotonic()
        process, log = start_server(server, root, url)
        samples: list[dict] = []
        failure = None
        timed_out = False
        resources = None
        try:
            deadline = started + timeout
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    failure = "server_exited"
                    break
                sample = query(url, SAMPLE_SQL)
                sample["elapsed_seconds"] = round(time.monotonic() - started, 3)
                samples.append(sample)
                if sample["total"] != items:
                    failure = "unexpected_work_item_count"
                    break
                if sample["missing"] == items and sample["scan"] is None:
                    break
                time.sleep(POLL_SECONDS)
            else:
                timed_out = True
            elapsed = time.monotonic() - started
            resources = server_resources(process.pid)
        finally:
            stop_server(process, log)
        verify_provenance(ROOT, server, provenance)
        success = bool(samples and samples[-1]["missing"] == items and samples[-1]["scan"] is None)
        print(json.dumps({"scenario": "missing-recovery-v1", "success": success,
            "timed_out": timed_out, "failure": failure, "elapsed_seconds": round(elapsed, 3),
            "censored": timed_out, "items": items, "fixture": shape, "sample_count": len(samples),
            "samples": samples, "server_resources": resources, "host": host_info(),
            "missing_retry_plan": retry_plan,
            "storage": storage, "server_sha256": file_digest(server), "build": provenance,
            "reconciliation_interval_seconds": 1, "poll_seconds": POLL_SECONDS,
            "limitations": ["empty inbound root; all seeded work items are missing",
                "only initial in-memory fixture is controlled; cache state is not controlled",
                "no media decoding, search rebuild, or interactive workload",
                "sampled state does not establish continuous lock duration or crash fairness",
                "server resource snapshot excludes child processes and is not a peak measurement"]}, sort_keys=True))
        return 0 if success and not timed_out and failure is None else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--items", type=int, default=35000)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--provenance", type=Path, required=True)
    parser.add_argument("--temp-parent", type=Path)
    args = parser.parse_args()
    try:
        return run(args.items, args.timeout, args.server, args.provenance, os.environ.copy(), args.temp_parent)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"Missing recovery benchmark failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
