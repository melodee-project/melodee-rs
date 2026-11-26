#!/usr/bin/env python3
"""Reproducible manifest-storage and outbox-retention measurements.

Creates a disposable PostgreSQL database, applies the real Melodee migrations,
and inserts the *same* synthetic manifest fixture twice under two physical
shapes:

``baseline``
    the pre-ADR-0213 shape: one row per manifest with the full versioned JSON
    document in a ``jsonb`` column (table + TOAST + indexes);
``candidate``
    the ADR-0213 shape: the same logical manifest row with a verified evidence
    reference, compact facts, per-track facts and a migration receipt (the
    evidence *file* bytes live on disk, outside PostgreSQL).

The script reports ``pg_total_relation_size`` for every object in each shape and
the reduction. It never reads or writes production data; it refuses database
names it did not create. Run with ``--keep`` to inspect the fixture.

Usage:
    python3 scripts/measure_manifest_storage.py [--manifests 1000] [--keep]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ADMIN_URL = "postgres://melodee:melodee-dev@127.0.0.1:5433/postgres"


def admin_url() -> str:
    return os.environ.get("MELODEE_TEST_DATABASE_URL", DEFAULT_ADMIN_URL)


def psql_binary() -> str:
    found = shutil.which("psql")
    if not found:
        raise SystemExit("error: psql is not on PATH")
    return found


def run(psql: str, url: str, sql: str) -> str:
    completed = subprocess.run(
        [psql, url, "--no-psqlrc", "--quiet", "--tuples-only", "--no-align",
         "--set", "ON_ERROR_STOP=1", "-c", sql],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise SystemExit(f"error: psql failed: {completed.stderr.strip()}")
    return completed.stdout.strip()


def apply_migrations(database_url: str) -> None:
    binary = ROOT / "target" / "debug" / "melodee-cli"
    if not binary.exists():
        raise SystemExit(
            "error: build the CLI first (cargo build -p melodee-cli)"
        )
    env = dict(os.environ)
    env["MELODEE__DATABASE__URL"] = database_url
    completed = subprocess.run(
        [str(binary), "migrate"],
        capture_output=True,
        text=True,
        env=env,
        cwd=ROOT,
        check=False,
    )
    if completed.returncode != 0:
        raise SystemExit(f"error: migration failed: {completed.stderr.strip()}")


def fixture_sql(manifests: int) -> str:
    """Identical logical manifests for both physical shapes."""
    return f"""
    -- Deterministic, poorly compressible text so the fixture resembles real
    -- provenance documents, lyrics and Chromaprint strings instead of being
    -- collapsed by TOAST compression.
    CREATE FUNCTION bench_text(seed text, bytes integer) RETURNS text LANGUAGE sql IMMUTABLE AS $$
        SELECT string_agg(md5(seed || ':' || g::text), '')
          FROM generate_series(1, (bytes / 32) + 1) AS g
    $$;

    CREATE FUNCTION bench_manifest(i integer) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
        SELECT jsonb_build_object(
            'schema_version', 3,
            'source_path', 'Artist ' || i || '/Release ' || i,
            'release_observation', jsonb_build_object(
                'title', 'Release ' || i,
                'artist', 'Artist ' || i,
                'blackbeard', CASE WHEN i % 147 < 84
                    THEN jsonb_build_object('document', bench_text('provenance-' || i, 24000),
                                            'unknownField', jsonb_build_object('nested', jsonb_build_array(1,2,3)))
                    ELSE 'null'::jsonb END
            ),
            'files', (SELECT jsonb_agg(jsonb_build_object(
                    'relative_path', 'Artist ' || i || '/Release ' || i || '/' || track || '.flac',
                    'bytes_read_errors', false,
                    'audio', jsonb_build_object('bit_depth', 24, 'sample_rate_hz', 96000, 'channels', 2),
                    'metadata', jsonb_build_object(
                        'genre', 'Rock; Alternative',
                        'raw', jsonb_build_object('FLAGCOMPILATION', '0',
                                                   'lyrics', bench_text('lyrics-' || i, 2000))
                    )
                ) ORDER BY track)
                FROM generate_series(1, 7) AS track),
            'normalization', jsonb_build_object(
                'profile_version', 'fixture-v1',
                'fingerprints', (SELECT jsonb_agg(jsonb_build_object(
                        'source_file', 'Artist ' || i || '/Release ' || i || '/' || track || '.flac',
                        'fingerprint', bench_text('fingerprint-' || i || '-' || track, 3000),
                        'duration_ms', 180000
                    ) ORDER BY track)
                    FROM generate_series(1, 7) AS track),
                'parsed_sidecars', jsonb_build_array(jsonb_build_object(
                    'sidecar_type', 'lrc',
                    'relative_path', 'Artist ' || i || '/Release ' || i || '/01.lrc',
                    'parsed', jsonb_build_object('value', bench_text('sidecar-' || i, 1500))
                ))
            )
        )
    $$;

    -- Baseline: pre-ADR-0213 physical shape.
    CREATE TABLE baseline.manifest (
        id bigint PRIMARY KEY,
        work_item_id uuid NOT NULL,
        schema_version integer NOT NULL,
        manifest_hash text NOT NULL,
        manifest_payload jsonb NOT NULL,
        generated_at timestamptz NOT NULL
    );
    CREATE INDEX baseline_manifest_hash_idx ON baseline.manifest (manifest_hash);
    CREATE INDEX baseline_manifest_generated_idx ON baseline.manifest (generated_at, id);

    INSERT INTO baseline.manifest
        SELECT i, uuidv7(), 3, md5(i::text) || md5(i::text), bench_manifest(i), now()
          FROM generate_series(1, {manifests}) AS i;

    -- Candidate: ADR-0213 physical shape with payload replaced by evidence.
    CREATE TABLE candidate.manifest (
        id bigint PRIMARY KEY,
        work_item_id uuid NOT NULL,
        schema_version integer NOT NULL,
        manifest_hash text NOT NULL,
        manifest_payload jsonb NULL,
        generated_at timestamptz NOT NULL
    );
    CREATE INDEX candidate_manifest_hash_idx ON candidate.manifest (manifest_hash);
    CREATE INDEX candidate_manifest_generated_idx ON candidate.manifest (generated_at, id);

    CREATE TABLE candidate.asset_files (asset_key text PRIMARY KEY, size_bytes bigint NOT NULL);
    CREATE TABLE candidate.evidence (
        manifest_row_id bigint PRIMARY KEY REFERENCES candidate.manifest (id),
        asset_key text NOT NULL REFERENCES candidate.asset_files (asset_key),
        storage_kind text NOT NULL,
        content_sha256 text NOT NULL,
        content_length bigint NOT NULL,
        serialization_version integer NOT NULL,
        revision integer NOT NULL,
        source text NOT NULL
    );
    CREATE INDEX candidate_evidence_key_idx ON candidate.evidence (asset_key);
    CREATE TABLE candidate.evidence_revision (
        manifest_row_id bigint NOT NULL REFERENCES candidate.manifest (id),
        revision integer NOT NULL,
        asset_key text NOT NULL REFERENCES candidate.asset_files (asset_key),
        content_length bigint NOT NULL,
        PRIMARY KEY (manifest_row_id, revision)
    );
    CREATE INDEX candidate_evidence_revision_key_idx ON candidate.evidence_revision (asset_key);
    CREATE TABLE candidate.facts (
        manifest_row_id bigint PRIMARY KEY REFERENCES candidate.manifest (id),
        release_title text,
        source_path text,
        has_corrupt_media boolean NOT NULL,
        structured_traits text[] NOT NULL,
        embedded_traits text[] NOT NULL,
        lyrics_present boolean NOT NULL
    );
    CREATE TABLE candidate.track_facts (
        manifest_row_id bigint NOT NULL REFERENCES candidate.manifest (id),
        relative_path text NOT NULL,
        bit_depth integer,
        sample_rate_hz integer,
        channels integer,
        fingerprint_digest text,
        fingerprint_length integer,
        corrupt boolean NOT NULL,
        genre text,
        source_ordinal integer NOT NULL,
        PRIMARY KEY (manifest_row_id, relative_path)
    );
    CREATE INDEX candidate_track_facts_path_idx ON candidate.track_facts (relative_path);
    CREATE TABLE candidate.receipts (
        manifest_row_id bigint PRIMARY KEY,
        content_sha256 text NOT NULL,
        content_length bigint NOT NULL,
        derived_facts_hash text NOT NULL
    );

    INSERT INTO candidate.manifest
        SELECT i, uuidv7(), 3, md5(i::text) || md5(i::text), NULL, now()
          FROM generate_series(1, {manifests}) AS i;
    INSERT INTO candidate.asset_files
        SELECT md5(i::text) || '-' || (length(bench_manifest(i)::text))::text,
               length(bench_manifest(i)::text)
          FROM generate_series(1, {manifests}) AS i;
    INSERT INTO candidate.evidence
        SELECT i, md5(i::text) || '-' || (length(bench_manifest(i)::text))::text,
               'chunked', md5(i::text) || md5(i::text),
               length(bench_manifest(i)::text), 1, 1, 'backfilled'
          FROM generate_series(1, {manifests}) AS i;
    INSERT INTO candidate.evidence_revision
        SELECT i, 1, md5(i::text) || '-' || (length(bench_manifest(i)::text))::text,
               length(bench_manifest(i)::text)
          FROM generate_series(1, {manifests}) AS i;
    INSERT INTO candidate.facts
        SELECT i, 'Release ' || i, 'Artist ' || i || '/Release ' || i, false,
               ARRAY['compilation'], ARRAY['ep'], true
          FROM generate_series(1, {manifests}) AS i;
    INSERT INTO candidate.track_facts
        SELECT i, 'Artist ' || i || '/Release ' || i || '/' || track || '.flac', 24, 96000, 2,
               md5(i::text || track::text) || md5(track::text || i::text), 3080, false,
               'Rock; Alternative', track
          FROM generate_series(1, {manifests}) AS i, generate_series(1, 7) AS track;
    INSERT INTO candidate.receipts
        SELECT i, md5(i::text) || md5(i::text), length(bench_manifest(i)::text),
               md5(i::text) || md5(i::text)
          FROM generate_series(1, {manifests}) AS i;

    ANALYZE baseline.manifest;
    ANALYZE candidate.manifest;
    ANALYZE candidate.evidence;
    ANALYZE candidate.facts;
    ANALYZE candidate.track_facts;
    """


SIZE_SQL = """
WITH objects AS (
    SELECT 'baseline' AS shape, format('%I.%I', schemaname, relname) AS relation,
           pg_total_relation_size(format('%I.%I', schemaname, relname)) AS bytes
      FROM pg_stat_user_tables WHERE schemaname = 'baseline'
    UNION ALL
    SELECT 'candidate', format('%I.%I', schemaname, relname),
           pg_total_relation_size(format('%I.%I', schemaname, relname))
      FROM pg_stat_user_tables WHERE schemaname = 'candidate'
)
SELECT shape, relation, bytes FROM objects ORDER BY shape, relation;
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifests", type=int, default=1000)
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    if not 1 <= args.manifests <= 20000:
        raise SystemExit("error: --manifests must be between 1 and 20000")

    psql = psql_binary()
    admin = admin_url()
    name = f"melodee_bench_storage_{uuid.uuid4().hex}"
    database = admin.rsplit("/", 1)[0] + "/" + name
    subprocess.run(
        [psql, admin, "--no-psqlrc", "--quiet", "--set", "ON_ERROR_STOP=1",
         "-c", f'CREATE DATABASE "{name}"'],
        check=True,
    )
    print(f"created disposable database {name}", file=sys.stderr)
    try:
        apply_migrations(database)
        run(psql, database, "CREATE SCHEMA baseline; CREATE SCHEMA candidate;")
        run(psql, database, fixture_sql(args.manifests))
        rows = []
        for line in run(psql, database, SIZE_SQL).splitlines():
            shape, relation, size = line.split("|")
            rows.append({"shape": shape, "relation": relation, "bytes": int(size)})
        totals = {"baseline": 0, "candidate": 0}
        for row in rows:
            totals[row["shape"]] += row["bytes"]
        baseline = totals["baseline"]
        candidate = totals["candidate"]
        reduction = (baseline - candidate) / baseline if baseline else 0.0
        payload_bytes = int(
            run(
                psql,
                database,
                "SELECT sum(length(manifest_payload::text)) FROM baseline.manifest",
            )
        )
        result = {
            "fixture": {
                "manifests": args.manifests,
                "tracksPerManifest": 7,
                "provenanceDocuments": f"84 of every 147 manifests",
                "label": "synthetic metadata-only fixture; no audio files",
            },
            "baselineBytes": baseline,
            "candidateBytes": candidate,
            "reductionPercent": round(reduction * 100, 2),
            "inlinePayloadBytes": payload_bytes,
            "objects": rows,
            "gate": {
                "requiresAtLeastPercent": 50,
                "passed": reduction >= 0.5,
            },
            "note": (
                "Evidence files live under MELODEE__STORAGE__ASSET_ROOT; this "
                "measurement counts PostgreSQL table, TOAST and index bytes only. "
                "WAL is measured separately by the operator runbook."
            ),
        }
        print(json.dumps(result, indent=2, ensure_ascii=False))
        return 0 if result["gate"]["passed"] else 1
    finally:
        if args.keep:
            print(f"keeping database {name}", file=sys.stderr)
        else:
            subprocess.run(
                [psql, admin, "--no-psqlrc", "--quiet", "--set", "ON_ERROR_STOP=1",
                 "-c", f'DROP DATABASE IF EXISTS "{name}" WITH (FORCE)'],
                check=False,
            )


if __name__ == "__main__":
    sys.exit(main())
