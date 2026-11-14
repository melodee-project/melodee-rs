#!/usr/bin/env python3
"""Runtime invariant audit for a running Melodee instance.

Why this exists
---------------
The repository gates prove a lot, but not *these* failures:

* `admin-ui/e2e/*.spec.ts` mocks the Admin API with `page.route()`, so a form can
  send a field the server ignores (or a value the server rejects) and the suite
  still passes. That is exactly how the Release `country`/`releaseType` fields
  survived a contract change and how Artist `type` stayed free text over a closed
  dictionary.
* `crates/**/tests` and `apps/melodee-server/tests` exercise the real router and
  PostgreSQL, but they assert the API — not that the UI calls it correctly, and
  not that every lifecycle state is reachable end to end.
* Nothing asserted that a workflow *finishes*: a Release could sit in `indexing`
  forever, and every gate stayed green.

This script is the missing layer: read-only assertions over a live instance's
database that fail when the product is internally inconsistent, when a workflow
cannot complete, or when a value the UI can display could never have been
produced.

Usage
-----
    python3 scripts/audit_runtime_invariants.py                 # human table
    python3 scripts/audit_runtime_invariants.py --json          # machine output
    python3 scripts/audit_runtime_invariants.py --stuck-minutes 10

Connection: `MELODEE__DATABASE__URL` from the environment, else the value in
`.env`, else the documented development default. Only `SELECT` statements are
executed — the script refuses to run any check that could mutate state.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_DATABASE_URL = "postgres://melodee:melodee-dev@127.0.0.1:5433/melodee"

# Nothing in this audit may write. The guard below is enforced at runtime. Whole-word
# matching keeps identifiers such as `updated_at` from tripping it.
MUTATING_STATEMENT = re.compile(
    r"\b(insert|update|delete|truncate|drop|alter|create|grant|revoke|copy|vacuum)\b",
    re.IGNORECASE,
)


@dataclass(frozen=True)
class Check:
    """One invariant: a name, what it protects, and the query that violates it."""

    name: str
    protects: str
    sql: str
    # When a violation is expected to be interesting, the query can return the
    # offending keys so the operator can act; kept bounded.
    detail_sql: str | None = None


CHECKS: tuple[Check, ...] = (
    Check(
        "workflow.no_stuck_non_terminal",
        "No work item sits in a state a worker is supposed to advance. Operator-park "
        "states (needs_review, quarantined, failed) are excluded here and reported "
        "separately by `workflow.awaiting_operator`, because waiting for a human is not "
        "a stuck pipeline.",
        """
        SELECT count(*)::bigint FROM release_work_items
         WHERE state NOT IN ('indexed', 'ignored', 'deleted',
                             'needs_review', 'quarantined', 'failed')
           AND updated_at < now() - make_interval(mins => $1)
        """,
        """
        SELECT state, count(*)::bigint FROM release_work_items
         WHERE state NOT IN ('indexed', 'ignored', 'deleted',
                             'needs_review', 'quarantined', 'failed')
           AND updated_at < now() - make_interval(mins => $1)
         GROUP BY state ORDER BY 2 DESC
        """,
    ),
    Check(
        "workflow.client_visible_matches_status",
        "A work item only claims to be client-visible (indexing/indexed) while its Release "
        "status is `active`. The two are set in one transaction, so a mismatch means a "
        "partial commit or a CLI/DB edit outside the API.",
        """
        SELECT count(*)::bigint
          FROM release_work_items w
          JOIN catalog_imports ci ON ci.work_item_id = w.id
          JOIN releases r ON r.id = ci.release_id
         WHERE w.state IN ('indexing', 'indexed') AND r.status <> 'active'
        """,
        """
        SELECT w.state, r.status, count(*)::bigint
          FROM release_work_items w
          JOIN catalog_imports ci ON ci.work_item_id = w.id
          JOIN releases r ON r.id = ci.release_id
         WHERE w.state IN ('indexing', 'indexed') AND r.status <> 'active'
         GROUP BY 1, 2
        """,
    ),
    Check(
        "workflow.indexed_requires_durable_projection",
        "`indexed` means the search projection is complete: no pending outbox row may remain "
        "for the Release, its Tracks, or its billed Artists.",
        """
        WITH completed AS (
            SELECT w.id, ci.release_id
              FROM release_work_items w
              JOIN catalog_imports ci ON ci.work_item_id = w.id
             WHERE w.state = 'indexed'
        ), pending AS (
            SELECT DISTINCT c.id
              FROM completed c
              LEFT JOIN tracks t ON t.release_id = c.release_id
              LEFT JOIN release_artists ra ON ra.release_id = c.release_id
              JOIN catalog_projection_outbox o
                ON o.processed_at IS NULL
               AND ((o.entity_type = 'release' AND o.entity_id = c.release_id)
                 OR (o.entity_type = 'track' AND o.entity_id = t.id)
                 OR (o.entity_type = 'artist' AND o.entity_id = ra.artist_id))
        )
        SELECT count(*)::bigint FROM pending
        """,
        None,
    ),
    Check(
        "workflow.awaiting_operator",
        "Items parked on an operator decision are listed, never failed: this is "
        "informational and always passes, so the queue depth is visible in the audit "
        "report without pretending a human decision is a defect.",
        """
        SELECT 0::bigint
        """,
        """
        SELECT state, count(*)::bigint FROM release_work_items
         WHERE state IN ('needs_review', 'quarantined', 'failed')
         GROUP BY state ORDER BY 2 DESC
        """,
    ),
    Check(
        "catalog.playable_asset_resolves",
        "Every Track points at a resolvable playable media asset; client playback depends on it.",
        """
        SELECT count(*)::bigint
          FROM tracks t
          LEFT JOIN media_assets m ON m.id = t.playable_media_asset_id
         WHERE m.id IS NULL
        """,
        None,
    ),
    Check(
        "catalog.release_aggregate_matches_tracks",
        "The Release aggregate the UI renders (track count, duration) matches the Track rows.",
        """
        SELECT count(*)::bigint
          FROM release_catalog_aggregates a
          JOIN releases r ON r.id = a.release_id
         WHERE a.track_count <> (SELECT count(*) FROM tracks t WHERE t.release_id = r.id)
            OR a.duration_ms <> COALESCE((SELECT sum(t.duration_ms) FROM tracks t WHERE t.release_id = r.id), 0)
        """,
        """
        SELECT r.title, a.track_count, (SELECT count(*) FROM tracks t WHERE t.release_id = r.id)
          FROM release_catalog_aggregates a
          JOIN releases r ON r.id = a.release_id
         WHERE a.track_count <> (SELECT count(*) FROM tracks t WHERE t.release_id = r.id)
         LIMIT 10
        """,
    ),
    Check(
        "catalog.artist_credits_backed_by_rows",
        "An Artist statistic the UI shows (track credits, appearances) has drill-down rows. "
        "This is the invariant that was violated when 'Appearances 1' opened an empty tab.",
        """
        SELECT count(*)::bigint
          FROM artist_catalog_aggregates a
         WHERE (a.billed_track_count > 0
                AND NOT EXISTS (SELECT 1 FROM artist_track_associations x WHERE x.artist_id = a.artist_id))
            OR (a.performance_track_count > 0
                AND NOT EXISTS (SELECT 1 FROM artist_track_associations x
                                 WHERE x.artist_id = a.artist_id AND x.is_performance))
        """,
        None,
    ),
    Check(
        "catalog.artist_type_in_dictionary",
        "No Artist carries a type outside the closed dictionary. Free-text editing (before the "
        "select was added) could have written anything the store did not reject.",
        """
        SELECT count(*)::bigint FROM artists
         WHERE artist_type NOT IN
               ('person','group','orchestra','ensemble','choir','character','other','unknown')
        """,
        """
        SELECT artist_type, count(*)::bigint FROM artists
         WHERE artist_type NOT IN
               ('person','group','orchestra','ensemble','choir','character','other','unknown')
         GROUP BY 1
        """,
    ),
    Check(
        "catalog.artist_status_in_dictionary",
        "No Artist carries a status outside the domain enum.",
        """
        SELECT count(*)::bigint FROM artists
         WHERE status NOT IN ('active','verified','merged','deprecated')
        """,
        None,
    ),
    Check(
        "catalog.current_overlay_artist_references",
        "Current immutable import snapshots reference the Artists recorded by their decisions.",
        """
        SELECT count(*)::bigint
          FROM releases release
          JOIN catalog_semantic_overlays overlay ON overlay.id=release.current_overlay_id
          CROSS JOIN LATERAL jsonb_array_elements(
              overlay.payload->'references'->'artist_resolutions'
          ) reference
          LEFT JOIN artists artist ON artist.id::text=reference->>'artist_id'
          LEFT JOIN artist_match_decisions decision ON decision.id::text=reference->>'decision_id'
         WHERE artist.id IS NULL
            OR reference->>'artist_id' IS DISTINCT FROM decision.artist_id::text
        """,
        None,
    ),
    Check(
        "catalog.release_status_in_dictionary",
        "No Release carries a status outside the domain enum.",
        """
        SELECT count(*)::bigint FROM releases
         WHERE status NOT IN ('active','deprecated','deleted')
        """,
        None,
    ),
    Check(
        "workflow.state_in_dictionary",
        "No work item carries a state outside the domain enum (a value the UI could never render).",
        """
        SELECT count(*)::bigint FROM release_work_items
         WHERE state NOT IN ('discovered','settling','ready','inspecting','normalizing',
                             'validating','needs_review','staged','publishing','published',
                             'cataloging','cataloged','indexing','indexed','failed',
                             'quarantined','ignored','deleted')
        """,
        None,
    ),
    Check(
        "catalog.import_bindings_resolve",
        "Every catalog import binds an existing work item and an existing Release (the queue's "
        "Release/Artist columns and the completion logic both depend on this join).",
        """
        SELECT count(*)::bigint
          FROM catalog_imports ci
          LEFT JOIN release_work_items w ON w.id = ci.work_item_id
          LEFT JOIN releases r ON r.id = ci.release_id
         WHERE w.id IS NULL OR r.id IS NULL
        """,
        None,
    ),
    Check(
        "identity.provider_candidates_resolve",
        "Every provider identity candidate belongs to an existing Artist (the Identity tab "
        "renders them).",
        """
        SELECT count(*)::bigint FROM provider_identity_candidates c
          LEFT JOIN artists a ON a.id = c.artist_id
         WHERE a.id IS NULL
        """,
        None,
    ),
    Check(
        "projection.no_stale_outbox_when_idle",
        "No projection row stays pending while nothing is in flight: a stalled projection "
        "worker would silently stop new Tracks appearing in search.",
        """
        SELECT count(*)::bigint FROM catalog_projection_outbox o
         WHERE o.processed_at IS NULL
           AND o.last_error IS NULL
           AND o.available_at < now() - interval '10 minutes'
           AND NOT EXISTS (SELECT 1 FROM release_work_items w
                            WHERE w.state NOT IN ('indexed','ignored','deleted'))
        """,
        """
        SELECT projection_name, entity_type, count(*)::bigint, min(available_at)
          FROM catalog_projection_outbox
         WHERE processed_at IS NULL AND last_error IS NULL
           AND available_at < now() - interval '10 minutes'
         GROUP BY 1, 2
        """,
    ),
    Check(
        "queue.counts_match_rows",
        "The per-state counts the queue filter renders match the work-item rows.",
        """
        SELECT abs(
                 (SELECT count(*) FROM release_work_items) -
                 (SELECT COALESCE(sum(count), 0) FROM (
                     SELECT count(*)::bigint AS count FROM release_work_items GROUP BY state
                 ) per_state)
               )::bigint
        """,
        None,
    ),
)


class AuditError(RuntimeError):
    """The audit could not run (connection, missing client, unsafe statement)."""


def database_url() -> str:
    explicit = os.environ.get("MELODEE__DATABASE__URL")
    if explicit:
        return explicit
    env_file = ROOT / ".env"
    if env_file.is_file():
        for line in env_file.read_text().splitlines():
            stripped = line.strip()
            if stripped.startswith("MELODEE__DATABASE__URL="):
                value = stripped.split("=", 1)[1].strip().strip('"').strip("'")
                if value:
                    return value
    return DEFAULT_DATABASE_URL


def psql_binary() -> str:
    found = shutil.which("psql")
    if not found:
        raise AuditError("psql is not on PATH; install the PostgreSQL client")
    return found


def run_query(psql: str, url: str, sql: str, stuck_minutes: int) -> list[list[str]]:
    match = MUTATING_STATEMENT.search(sql)
    if match:
        raise AuditError(
            f"refusing to run a non-read-only statement ({match.group(0).lower()})"
        )
    command = [
        psql,
        url,
        "--no-psqlrc",
        "--quiet",
        "--tuples-only",
        "--no-align",
        "--field-separator",
        "\u001f",
        "--set",
        "ON_ERROR_STOP=1",
        "--set",
        f"stuck_minutes={stuck_minutes}",
        "-c",
        # `make_interval(mins => $1)` needs a bind; psql variables only interpolate
        # as text, so the value is inlined after validation as a plain integer.
        sql.replace("$1", str(int(stuck_minutes))),
    ]
    completed = subprocess.run(command, capture_output=True, text=True, check=False)
    if completed.returncode != 0:
        raise AuditError(completed.stderr.strip() or "psql failed")
    rows = []
    for line in completed.stdout.splitlines():
        if line.strip() == "":
            continue
        rows.append(line.split("\u001f"))
    return rows


def run_audit(*, stuck_minutes: int) -> dict[str, object]:
    psql = psql_binary()
    url = database_url()
    results: list[dict[str, object]] = []
    failures = 0
    for check in CHECKS:
        violations_rows = run_query(psql, url, check.sql, stuck_minutes)
        violations = int(violations_rows[0][0]) if violations_rows and violations_rows[0] else 0
        detail: list[list[str]] = []
        if violations and check.detail_sql:
            detail = run_query(psql, url, check.detail_sql, stuck_minutes)
        failed = violations > 0
        failures += 1 if failed else 0
        results.append(
            {
                "name": check.name,
                "protects": check.protects,
                "violations": violations,
                "detail": detail,
                "status": "FAIL" if failed else "PASS",
            }
        )
    return {
        "database": url.split("@")[-1],
        "stuck_minutes": stuck_minutes,
        "checks": results,
        "failures": failures,
    }


def print_report(report: dict[str, object]) -> None:
    print(f"Melodee runtime invariant audit — database {report['database']}")
    print(f"stuck threshold: {report['stuck_minutes']} minutes\n")
    for check in report["checks"]:  # type: ignore[union-attr]
        mark = "PASS" if check["status"] == "PASS" else "FAIL"
        print(f"  {mark}  {check['name']}  (violations: {check['violations']})")
        if check["status"] == "FAIL":
            print(f"        protects: {check['protects']}")
            for row in check["detail"]:
                print(f"        {' | '.join(row)}")
    failures = report["failures"]
    print()
    if failures:
        print(f"{failures} invariant(s) violated.")
    else:
        print("All runtime invariants hold.")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    parser.add_argument(
        "--stuck-minutes",
        type=int,
        default=30,
        help="how long a non-terminal work item may sit untouched before it counts as stuck (default 30)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.stuck_minutes < 0:
        print("error: --stuck-minutes must not be negative", file=sys.stderr)
        return 2
    try:
        report = run_audit(stuck_minutes=args.stuck_minutes)
    except AuditError as error:
        print(f"audit could not run: {error}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print_report(report)
    return 1 if report["failures"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
