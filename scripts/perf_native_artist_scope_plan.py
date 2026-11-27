#!/usr/bin/env python3
"""Compare native ACL-scoped Artist count queries on an owned catalog clone.

Run as a direct child of perf_representative.py. The original fixture is never
modified. Registered clones are removed on normal exit and handled signals;
SIGKILL or an ambiguous CREATE DATABASE result may need operator inspection.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import sys
import uuid

from build_perf_server import file_digest
from perf_jellyfin_audio_plan import check_parent
import test_database

IDS = "ARRAY(SELECT id FROM artists ORDER BY sort_name, id LIMIT 50)"
USER = "'00000000-0000-0000-0000-000000000000'::uuid"


def query(scoped: bool) -> str:
    release_filter = f" AND release_artist.artist_id = ANY({IDS})" if scoped else ""
    track_filter = f" WHERE association.artist_id = ANY({IDS})" if scoped else ""
    return f"""SELECT artist.id, COALESCE(releases.album_count, 0)::bigint,
        COALESCE(tracks.song_count, 0)::bigint
      FROM artists artist
      LEFT JOIN (
        SELECT release_artist.artist_id, count(DISTINCT release_artist.release_id)::bigint AS album_count
        FROM release_artists release_artist JOIN releases release ON release.id=release_artist.release_id
          AND release.status='active' AND native_user_can_access_release({USER},release.id)
        WHERE release_artist.role='primary'{release_filter}
        GROUP BY release_artist.artist_id
      ) releases ON releases.artist_id=artist.id
      LEFT JOIN (
        SELECT association.artist_id, count(DISTINCT association.track_id)::bigint AS song_count
        FROM artist_track_associations association JOIN tracks track ON track.id=association.track_id
        JOIN releases release ON release.id=track.release_id AND release.status='active'
          AND native_user_can_access_release({USER},release.id){track_filter}
        GROUP BY association.artist_id
      ) tracks ON tracks.artist_id=artist.id
      WHERE artist.id=ANY({IDS}) ORDER BY artist.id"""


def run(env: dict[str, str], output: Path, artists: int, releases: int, tracks: int, iterations: int) -> None:
    if min(artists, releases, tracks, iterations) < 1 or artists > 1_000_000 or releases > 1_000_000 or tracks > 100 or iterations > 20:
        raise ValueError("fixture dimensions or iterations out of bounds")
    admin, source, source_url = check_parent(env, os.getppid())
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    if test_database.sql(source_url, "SELECT current_database()") != source:
        raise ValueError("source fixture identity mismatch")
    shape = json.loads(test_database.sql(source_url, """SELECT json_build_object(
        'artists',(SELECT count(*) FROM artists), 'releases',(SELECT count(*) FROM releases),
        'tracks',(SELECT count(*) FROM tracks), 'work',(SELECT count(*) FROM release_work_items),
        'jobs',(SELECT count(*) FROM search_rebuild_jobs))"""))
    if shape != {"artists": artists, "releases": releases, "tracks": releases * tracks, "work": 0, "jobs": 0}:
        raise ValueError("fixture shape differs or contains existing work")
    clone = "melodee_template_" + uuid.uuid4().hex
    if not test_database.OWNED.fullmatch(clone) or clone == source:
        raise ValueError("invalid clone name")
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
        results = {}
        for label, sql in (("before", query(False)), ("after", query(True))):
            samples = []
            for _ in range(iterations):
                plan = json.loads(test_database.sql(url, "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) " + sql))[0]
                rows = test_database.sql(url, "SELECT coalesce(json_agg(row_to_json(q)),'[]'::json)::text FROM (" + sql + ") q")
                samples.append({"plan": plan, "rows_sha256": hashlib.sha256(rows.encode()).hexdigest(),
                                "row_count": len(json.loads(rows))})
            results[label] = samples
        if len({sample["rows_sha256"] for samples in results.values() for sample in samples}) != 1:
            raise RuntimeError("scoped and original counts differ")
        report = {"fixture": shape, "iterations": iterations, "selected_artists": min(50, artists),
                  "queries": {"before": query(False), "after": query(True)}, "results": results,
                  "fixture_sha256": env["MELODEE_PERF_FIXTURE_SHA256"],
                  "migration_cli_sha256": env.get("MELODEE_PERF_MIGRATION_CLI_SHA256"),
                  "probe_sha256": file_digest(Path(__file__)),
                  "limitations": ["SQL plans only; not an end-to-end API benchmark",
                      "synthetic catalog has no import provenance or restricted releases",
                      "ACL predicate executed but this dataset cannot measure restricted-library selectivity",
                      "one host, serial ordering and uncontrolled cache state"]}
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
    parser.add_argument("--iterations", type=int, default=3)
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
        print(f"Native Artist scope plan probe failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
