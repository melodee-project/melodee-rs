#!/usr/bin/env python3
"""Compare Jellyfin Audio query plans on an invocation-owned catalog clone.

Invoke only as a direct child of perf_representative.py. The parent fixture is
never changed. A newly created, registered clone is dropped on normal exit or
handled SIGINT/SIGTERM; SIGKILL, host failure, and an uncertain CREATE DATABASE
acknowledgment may require operator inspection. Output contains no credentials.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import sys
import time
import uuid

from build_perf_server import file_digest
from run_perf_mutation_benchmarks import validate_fixture
import test_database

ROOT = Path(__file__).resolve().parents[1]
INDEX = "tracks_jellyfin_number_public_idx"
INDEX_SQL = f"CREATE INDEX {INDEX} ON tracks (track_number, public_api_id)"
INDEX_DEFINITION = f"CREATE INDEX {INDEX} ON public.tracks USING btree (track_number, public_api_id)"
ARTIST_NAME = """(SELECT artist.canonical_name FROM release_artists billing
 JOIN artists artist ON artist.id=billing.artist_id WHERE billing.release_id=release.id
 AND billing.role='primary' ORDER BY billing.presentation_order LIMIT 1)"""
ARTIST_ID = """(SELECT artist.public_api_id FROM release_artists billing
 JOIN artists artist ON artist.id=billing.artist_id WHERE billing.release_id=release.id
 AND billing.role='primary' ORDER BY billing.presentation_order LIMIT 1)"""
PAGE = f"""SELECT track.public_api_id, track.title, track.sort_title, track.track_number,
 track.duration_ms, media.container, media.byte_size, media.bitrate_bps, media.sample_rate_hz,
 release.title, release.public_api_id, track.created_at, track.updated_at, {ARTIST_NAME}, {ARTIST_ID}
 FROM tracks track JOIN releases release ON release.id=track.release_id AND release.status='active'
 JOIN media_assets media ON media.id=track.playable_media_asset_id
 ORDER BY 4, track.public_api_id LIMIT 25 OFFSET 0"""
COUNT = """SELECT count(*) FROM tracks track JOIN releases release
 ON release.id=track.release_id AND release.status='active'"""


def check_parent(env: dict[str, str], parent_pid: int) -> tuple[str, str, str]:
    validate_fixture(env)
    name = env["MELODEE_PERF_OWNED_DATABASE"]
    source_url = env["MELODEE__DATABASE__URL"]
    admin = env.get("MELODEE_TEST_DATABASE_URL", "")
    if (not admin or test_database.database_url(admin, name) != source_url
            or env.get("MELODEE_PERF_FIXTURE_PID") != str(parent_pid)):
        raise ValueError("fixture parent or database topology mismatch")
    for env_key, relative in (("MELODEE_PERF_FIXTURE_SHA256", "scripts/perf_representative.py"),
                              ("MELODEE_PERF_DATABASE_HELPER_SHA256", "scripts/test_database.py")):
        if env.get(env_key) != file_digest(ROOT / relative):
            raise ValueError("fixture or ownership helper changed")
    return admin, name, source_url


def measure(url: str, iterations: int) -> list[dict]:
    samples = []
    for _ in range(iterations):
        plans = {label: json.loads(test_database.sql(url, "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) " + sql))[0]
                 for label, sql in (("count", COUNT), ("page", PAGE))}
        rows = test_database.sql(url, "SELECT coalesce(json_agg(row_to_json(q)),'[]'::json)::text FROM (" + PAGE + ") q")
        samples.append({"plans": plans, "page_sha256": hashlib.sha256(rows.encode()).hexdigest(),
                        "exact_count": int(test_database.sql(url, COUNT))})
    return samples


def run(env: dict[str, str], output: Path, artists: int, releases: int,
        tracks_per_release: int, iterations: int) -> None:
    if (min(artists, releases, tracks_per_release, iterations) < 1 or artists > 1_000_000
            or releases > 1_000_000 or tracks_per_release > 100 or iterations > 20):
        raise ValueError("fixture dimensions or iterations out of bounds")
    admin, source, source_url = check_parent(env, os.getppid())
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    if test_database.sql(source_url, "SELECT current_database()") != source:
        raise ValueError("source fixture database mismatch")
    shape = json.loads(test_database.sql(source_url, """SELECT json_build_object(
        'artists',(SELECT count(*) FROM artists), 'releases',(SELECT count(*) FROM releases),
        'tracks',(SELECT count(*) FROM tracks), 'work',(SELECT count(*) FROM release_work_items),
        'jobs',(SELECT count(*) FROM search_rebuild_jobs))"""))
    expected = {"artists": artists, "releases": releases, "tracks": releases * tracks_per_release,
                "work": 0, "jobs": 0}
    if shape != expected:
        raise ValueError("fixture shape differs or contains existing work")
    clone = "melodee_template_" + uuid.uuid4().hex
    if not test_database.OWNED.fullmatch(clone) or clone == source:
        raise ValueError("invalid new clone name")
    created: set[str] = set()
    try:
        try:
            test_database.sql(admin, f'CREATE DATABASE "{clone}" TEMPLATE "{source}"')
        except BaseException:
            print(json.dumps({"uncertain_clone_create": clone}), file=sys.stderr, flush=True)
            raise
        created.add(clone)
        url = test_database.database_url(admin, clone)
        if test_database.sql(url, "SELECT current_database()") != clone:
            raise RuntimeError("clone database identity mismatch")
        existing = test_database.sql(url, f"SELECT coalesce(pg_get_indexdef(to_regclass('public.{INDEX}')), '')")
        if existing and existing != INDEX_DEFINITION:
            raise RuntimeError("candidate index definition differs; refusing to drop it")
        if existing:
            test_database.sql(url, f"DROP INDEX public.{INDEX}")
        before = measure(url, iterations)
        start = time.monotonic()
        test_database.sql(url, INDEX_SQL)
        build_seconds = time.monotonic() - start
        size = int(test_database.sql(url, f"SELECT pg_relation_size('public.{INDEX}')"))
        after = measure(url, iterations)
        if len({sample["page_sha256"] for sample in before + after}) != 1 or len(
                {sample["exact_count"] for sample in before + after}) != 1:
            raise RuntimeError("query results changed during comparison")
        report = {"fixture": shape, "iterations": iterations, "index_definition": INDEX_SQL,
                  "index_build_seconds": round(build_seconds, 6), "index_bytes": size,
                  "runs": {"before": before, "after": after},
                  "fixture_sha256": env["MELODEE_PERF_FIXTURE_SHA256"],
                  "migration_cli_sha256": env.get("MELODEE_PERF_MIGRATION_CLI_SHA256"),
                  "probe_sha256": file_digest(Path(__file__)),
                  "limitations": ["SQL plans only, not end-to-end HTTP", "one host, uncontrolled caches and sequential order",
                                  "unfiltered first page only; no write overhead or deep-offset measurements"]}
        with output.open("x", encoding="utf-8") as destination:
            json.dump(report, destination, indent=2, sort_keys=True)
            destination.write("\n")
    finally:
        for name in list(created):
            test_database.drop_owned(admin, name, created)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--artists", type=int, default=35000)
    parser.add_argument("--releases", type=int, default=50000)
    parser.add_argument("--tracks-per-release", type=int, default=12)
    parser.add_argument("--iterations", type=int, default=4)
    args = parser.parse_args()
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        run(dict(os.environ), args.output, args.artists, args.releases, args.tracks_per_release, args.iterations)
    except (ValueError, RuntimeError, OSError, InterruptedError) as error:
        print(f"Audio plan probe failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
