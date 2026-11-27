#!/usr/bin/env python3
"""Add synthetic identity relationships to an invocation-owned catalog fixture.

This destructive-to-fixture augmentation is only valid on an untouched database
created by perf_representative.py; it refuses reruns and non-owned databases.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys

from run_perf_mutation_benchmarks import validate_fixture

ROOT = Path(__file__).resolve().parents[1]

IDENTITY_SQL = r"""
BEGIN;
DO $$ BEGIN
 IF (SELECT count(*) FROM observed_artists) <> 0
 OR (SELECT count(*) FROM artist_match_decisions) <> 0
 OR (SELECT count(*) FROM artist_match_current) <> 0
 OR (SELECT count(*) FROM artist_external_identities) <> 0
 OR (SELECT count(*) FROM release_external_identities) <> 0 THEN
   RAISE EXCEPTION 'identity augmentation requires an untouched fixture';
 END IF;
 IF EXISTS (SELECT 1 FROM releases r WHERE NOT EXISTS
     (SELECT 1 FROM release_artists c WHERE c.release_id=r.id AND c.role='primary')) THEN
   RAISE EXCEPTION 'fixture requires a primary Artist for each Release';
 END IF;
 IF (SELECT count(*) FROM track_artists) <> (SELECT count(*) FROM tracks)
 OR EXISTS (
     SELECT 1 FROM tracks t
     JOIN release_artists release_credit ON release_credit.release_id=t.release_id AND release_credit.role='primary'
     LEFT JOIN track_artists credit ON credit.track_id=t.id AND credit.role='primary'
     WHERE credit.id IS NULL OR credit.artist_id <> release_credit.artist_id
        OR credit.observed_artist_id IS NOT NULL OR credit.presentation_order <> 1
 ) THEN
   RAISE EXCEPTION 'fixture requires one matching unobserved primary credit per Track';
 END IF;
END $$;

-- Preserve the first fixture Artists used by the workload. Active and
-- verified identities are mixed, and small same-name verified groups remain
-- distinct. Names remain display/search values, not identity keys.
UPDATE artists SET status=CASE WHEN substring(canonical_name FROM '[0-9]+$')::bigint % 4 = 0
                               THEN 'active' ELSE 'verified' END
WHERE canonical_name ~ '^Artist [0-9]+$';
UPDATE artists SET canonical_name='Benchmark Shared ' || (substring(canonical_name FROM '[0-9]+$')::bigint / 100),
                   normalized_name='benchmark shared ' || (substring(canonical_name FROM '[0-9]+$')::bigint / 100),
                   sort_name='Benchmark Shared ' || (substring(canonical_name FROM '[0-9]+$')::bigint / 100),
                   status='verified'
WHERE canonical_name ~ '^Artist [0-9]+$'
  AND substring(canonical_name FROM '[0-9]+$')::bigint >= 100
  AND substring(canonical_name FROM '[0-9]+$')::bigint % 100 < 5;

CREATE TEMP TABLE bench_identity_observations ON COMMIT DROP AS
SELECT uuidv7() AS observation_id, uuidv7() AS decision_id, r.id AS release_id,
       NULL::uuid AS track_id, 'release'::text AS scope, 0 AS source_ordinal,
       a.id AS artist_id, a.canonical_name AS display_name, a.normalized_name
FROM releases r JOIN release_artists credit ON credit.release_id=r.id AND credit.role='primary'
JOIN artists a ON a.id=credit.artist_id
UNION ALL
SELECT uuidv7(), uuidv7(), t.release_id, t.id, 'track', t.ordinal,
       a.id, a.canonical_name, a.normalized_name
FROM tracks t JOIN release_artists credit ON credit.release_id=t.release_id AND credit.role='primary'
JOIN artists a ON a.id=credit.artist_id;

INSERT INTO observed_artists (id,release_id,track_id,scope,source_ordinal,raw_value,normalized_value,display_value,source_pointer,source_payload,created_at)
SELECT observation_id,release_id,track_id,scope,source_ordinal,display_name,normalized_name,display_name,
       'fixture:identity:' || scope,'{}'::jsonb,now() FROM bench_identity_observations;
INSERT INTO artist_match_decisions (id,observed_artist_id,artist_id,outcome,authority,confidence,reason,is_locked,payload,created_at)
SELECT decision_id,observation_id,artist_id,'confirmed','automated',9500,'synthetic fixture',false,'{}'::jsonb,now()
FROM bench_identity_observations;
INSERT INTO artist_match_current (observed_artist_id,decision_id,updated_at)
SELECT observation_id,decision_id,now() FROM bench_identity_observations;
UPDATE release_artists credit SET observed_artist_id=observation.observation_id
FROM bench_identity_observations observation
WHERE observation.scope='release' AND observation.release_id=credit.release_id AND credit.role='primary';
UPDATE track_artists credit SET observed_artist_id=observation.observation_id,
                                display_value=observation.display_name
FROM bench_identity_observations observation
WHERE observation.scope='track' AND observation.track_id=credit.track_id AND credit.role='primary';

-- Deterministic unique provider keys. These are synthetic shape-compatible
-- evidence, not claims about real provider identities.
INSERT INTO artist_external_identities (id,artist_id,entity_type,provider,external_id,provider_unique_scope,confidence,evidence_source,is_locked,created_at)
SELECT uuidv7(),id,'artist','musicbrainz',
       '00000000-0000-7000-8000-' || lpad(to_hex(substring(canonical_name FROM '[0-9]+$')::bigint),12,'0'),
       false,9500,'synthetic-fixture',false,now()
FROM artists WHERE canonical_name ~ '^Artist [0-9]+$'
AND substring(canonical_name FROM '[0-9]+$')::bigint % 3 = 0;
INSERT INTO release_external_identities (id,release_id,provider,external_id,trusted,confidence,evidence_source,is_locked,observed_at)
SELECT uuidv7(),id,'musicbrainz',
       '10000000-0000-7000-8000-' || lpad(to_hex(substring(title FROM '[0-9]+$')::bigint),12,'0'),
       true,9500,'synthetic-fixture',false,now()
FROM releases WHERE title ~ '^Release [0-9]+$'
AND substring(title FROM '[0-9]+$')::bigint % 3 = 0;
COMMIT;
ANALYZE artists;
ANALYZE artist_external_identities;
ANALYZE release_external_identities;
ANALYZE observed_artists;
ANALYZE artist_match_decisions;
ANALYZE artist_match_current;
ANALYZE release_artists;
ANALYZE track_artists;
SELECT 'identity_fixture_counts',
 (SELECT count(*) FROM artists) AS artists,
 (SELECT count(*) FROM artists WHERE status='active') AS active_artists,
 (SELECT count(*) FROM observed_artists WHERE scope='release') AS release_observations,
 (SELECT count(*) FROM observed_artists WHERE scope='track') AS track_observations,
 (SELECT count(*) FROM artist_match_current) AS current_decisions,
 (SELECT count(*) FROM release_artists) AS release_credits,
 (SELECT count(*) FROM track_artists) AS track_credits,
 (SELECT count(*) FROM artist_external_identities) AS artist_external_ids,
 (SELECT count(*) FROM release_external_identities) AS release_external_ids,
 (SELECT count(*) FROM (SELECT normalized_name FROM artists GROUP BY normalized_name HAVING count(*) > 1) shared_groups) AS shared_name_groups;
"""


def main() -> int:
    argparse.ArgumentParser(description=__doc__).parse_args()
    try:
        env = os.environ.copy()
        validate_fixture(env)
        return subprocess.run(
            ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", env["MELODEE__DATABASE__URL"]],
            input=IDENTITY_SQL, text=True, cwd=ROOT, env=env, check=False,
        ).returncode
    except (OSError, ValueError) as error:
        print(f"Identity fixture augmentation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
