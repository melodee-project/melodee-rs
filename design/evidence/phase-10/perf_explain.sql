-- Phase 10 W4-5 performance evidence: EXPLAIN (ANALYZE, BUFFERS) for every
-- admin list-endpoint query, run against the synthetic catalog-scale scratch
-- dataset (200k work items, 100k artists, 100k releases, 500k tracks, 200k
-- observed artists, 300k audit events). SQL mirrors the store queries in
-- crates/melodee-persistence/src/{work_items,catalog_browse,identity_review,
-- user_admin,group_admin}.rs verbatim (binds inlined as literals).
\set QUIET on
\timing off

\echo '=== W1. work-items list_page: state filter + addedAt:desc (default), page 1'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM release_work_items
WHERE ('needs_review'::text IS NULL OR state = 'needs_review')
ORDER BY created_at DESC, id
LIMIT 50 OFFSET 0;

\echo '=== W1b. same query, deep page (OFFSET 10000)'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM release_work_items
WHERE ('needs_review'::text IS NULL OR state = 'needs_review')
ORDER BY created_at DESC, id
LIMIT 50 OFFSET 10000;

\echo '=== W2. work-items list_page: no filter + updatedAt:desc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM release_work_items
WHERE (NULL::text IS NULL OR state = NULL)
ORDER BY updated_at DESC, id
LIMIT 50 OFFSET 0;

\echo '=== W3. work-items list_page: no filter + attemptCount:desc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM release_work_items
WHERE (NULL::text IS NULL OR state = NULL)
ORDER BY attempt_count DESC, id
LIMIT 50 OFFSET 0;

\echo '=== W4. work-items list_page: state filter + state:asc sort'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM release_work_items
WHERE ('failed'::text IS NULL OR state = 'failed')
ORDER BY state ASC, created_at DESC, id
LIMIT 50 OFFSET 0;

\echo '=== W5. review queue list_page_states: 3-state ANY + addedAt:desc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM release_work_items
WHERE state = ANY('{needs_review,failed,quarantined}')
ORDER BY created_at DESC, id
LIMIT 50 OFFSET 0;

\echo '=== W6. work-items count (state filter)'
EXPLAIN (ANALYZE, BUFFERS)
SELECT COUNT(*)::bigint FROM release_work_items
WHERE ('needs_review'::text IS NULL OR state = 'needs_review');

\echo '=== W7. counts_by_state (GROUP BY state)'
EXPLAIN (ANALYZE, BUFFERS)
SELECT state, COUNT(*)::bigint FROM release_work_items GROUP BY state ORDER BY state;

\echo '=== W8. active leases listing'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM release_work_items
WHERE lease_expires_at IS NOT NULL AND lease_expires_at > now()
ORDER BY lease_expires_at DESC, id
LIMIT 50;

\echo '=== W9. users list: ILIKE filter + username:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM native_users
WHERE ('%user1%'::text IS NULL OR username ILIKE '%user1%' ESCAPE '\' OR email ILIKE '%user1%' ESCAPE '\')
ORDER BY normalized_username ASC, id LIMIT 50 OFFSET 0;

\echo '=== W10. users list: no filter + createdAt:desc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM native_users
WHERE (NULL::text IS NULL OR username ILIKE NULL OR email ILIKE NULL)
ORDER BY created_at DESC, id LIMIT 50 OFFSET 0;

\echo '=== A1. catalog artists: no filter + name:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT a.public_api_id, a.canonical_name, a.sort_name, a.artist_type,
       a.disambiguation, a.status,
       COALESCE(aggregate.release_count, 0) AS release_count,
       COALESCE(aggregate.all_credit_track_count, 0) AS track_count,
       a.created_at, a.updated_at
  FROM artists a
  LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = a.id
 WHERE (NULL::text IS NULL OR a.canonical_name ILIKE NULL ESCAPE '\'
       OR a.sort_name ILIKE NULL ESCAPE '\'
       OR EXISTS (SELECT 1 FROM artist_aliases alias
                  WHERE alias.artist_id = a.id
                    AND alias.alias ILIKE NULL ESCAPE '\'))
 ORDER BY a.sort_name ASC, a.id
 LIMIT 50 OFFSET 0;

\echo '=== A2. catalog artists: ILIKE substring filter + name:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT a.public_api_id, a.canonical_name, a.sort_name, a.artist_type,
       a.disambiguation, a.status,
       COALESCE(aggregate.release_count, 0) AS release_count,
       COALESCE(aggregate.all_credit_track_count, 0) AS track_count,
       a.created_at, a.updated_at
  FROM artists a
  LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = a.id
 WHERE ('%artist 99%'::text IS NULL OR a.canonical_name ILIKE '%artist 99%' ESCAPE '\'
       OR a.sort_name ILIKE '%artist 99%' ESCAPE '\'
       OR EXISTS (SELECT 1 FROM artist_aliases alias
                  WHERE alias.artist_id = a.id
                    AND alias.alias ILIKE '%artist 99%' ESCAPE '\'))
 ORDER BY a.sort_name ASC, a.id
 LIMIT 50 OFFSET 0;

\echo '=== A3. catalog artists: no filter + updatedAt:desc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT a.public_api_id, a.canonical_name, a.sort_name, a.artist_type,
       a.disambiguation, a.status,
       COALESCE(aggregate.release_count, 0) AS release_count,
       COALESCE(aggregate.all_credit_track_count, 0) AS track_count,
       a.created_at, a.updated_at
  FROM artists a
  LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = a.id
 WHERE (NULL::text IS NULL OR a.canonical_name ILIKE NULL ESCAPE '\'
       OR a.sort_name ILIKE NULL ESCAPE '\'
       OR EXISTS (SELECT 1 FROM artist_aliases alias
                  WHERE alias.artist_id = a.id
                    AND alias.alias ILIKE NULL ESCAPE '\'))
 ORDER BY a.updated_at DESC, a.id
 LIMIT 50 OFFSET 0;

\echo '=== R1. catalog releases: no filter + title:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT r.public_api_id, r.title, r.display_artist, r.release_date,
       r.release_date_text, r.status,
       COALESCE(aggregate.track_count, 0) AS track_count,
       COALESCE(aggregate.duration_ms, 0) AS duration_ms,
       r.created_at, r.updated_at
  FROM releases r
  LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = r.id
 WHERE (NULL::text IS NULL OR r.title ILIKE NULL ESCAPE '\')
   AND (NULL::uuid IS NULL OR EXISTS (SELECT 1 FROM release_artists billing
                                   JOIN artists billed ON billed.id = billing.artist_id
                                   WHERE billing.release_id = r.id
                                     AND billed.public_api_id = NULL))
 ORDER BY r.sort_title ASC, r.id
 LIMIT 50 OFFSET 0;

\echo '=== R2. catalog releases: ILIKE filter + title:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT r.public_api_id, r.title, r.display_artist, r.release_date,
       r.release_date_text, r.status,
       COALESCE(aggregate.track_count, 0) AS track_count,
       COALESCE(aggregate.duration_ms, 0) AS duration_ms,
       r.created_at, r.updated_at
  FROM releases r
  LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = r.id
 WHERE ('%release 77%'::text IS NULL OR r.title ILIKE '%release 77%' ESCAPE '\')
   AND (NULL::uuid IS NULL OR EXISTS (SELECT 1 FROM release_artists billing
                                   JOIN artists billed ON billed.id = billing.artist_id
                                   WHERE billing.release_id = r.id
                                     AND billed.public_api_id = NULL))
 ORDER BY r.sort_title ASC, r.id
 LIMIT 50 OFFSET 0;

\echo '=== R3. catalog releases: artistId EXISTS filter + title:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT r.public_api_id, r.title, r.display_artist, r.release_date,
       r.release_date_text, r.status,
       COALESCE(aggregate.track_count, 0) AS track_count,
       COALESCE(aggregate.duration_ms, 0) AS duration_ms,
       r.created_at, r.updated_at
  FROM releases r
  LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = r.id
 WHERE (NULL::text IS NULL OR r.title ILIKE NULL ESCAPE '\')
   AND ((SELECT public_api_id FROM artists ORDER BY id OFFSET 5 LIMIT 1)::uuid IS NULL
        OR EXISTS (SELECT 1 FROM release_artists billing
                   JOIN artists billed ON billed.id = billing.artist_id
                   WHERE billing.release_id = r.id
                     AND billed.public_api_id = (SELECT public_api_id FROM artists ORDER BY id OFFSET 5 LIMIT 1)))
 ORDER BY r.sort_title ASC, r.id
 LIMIT 50 OFFSET 0;

\echo '=== R4. catalog releases: no filter + releaseDate:desc NULLS LAST'
EXPLAIN (ANALYZE, BUFFERS)
SELECT r.public_api_id, r.title, r.display_artist, r.release_date,
       r.release_date_text, r.status,
       COALESCE(aggregate.track_count, 0) AS track_count,
       COALESCE(aggregate.duration_ms, 0) AS duration_ms,
       r.created_at, r.updated_at
  FROM releases r
  LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = r.id
 WHERE (NULL::text IS NULL OR r.title ILIKE NULL ESCAPE '\')
   AND (NULL::uuid IS NULL OR EXISTS (SELECT 1 FROM release_artists billing
                                   JOIN artists billed ON billed.id = billing.artist_id
                                   WHERE billing.release_id = r.id
                                     AND billed.public_api_id = NULL))
 ORDER BY r.release_date DESC NULLS LAST, r.id
 LIMIT 50 OFFSET 0;

\echo '=== T1. catalog tracks: no filter + title:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT t.public_api_id, t.title, t.display_artist, t.ordinal,
       t.disc_number, t.track_number, t.duration_ms,
       r.public_api_id AS release_public_api_id, r.title AS release_title,
       t.created_at
  FROM tracks t
  JOIN releases r ON r.id = t.release_id
 WHERE (NULL::text IS NULL OR t.title ILIKE NULL ESCAPE '\')
   AND (NULL::uuid IS NULL OR r.public_api_id = NULL)
 ORDER BY t.sort_title ASC, t.id
 LIMIT 50 OFFSET 0;

\echo '=== T2. catalog tracks: no filter + durationMs:desc (NO covering index)'
EXPLAIN (ANALYZE, BUFFERS)
SELECT t.public_api_id, t.title, t.display_artist, t.ordinal,
       t.disc_number, t.track_number, t.duration_ms,
       r.public_api_id AS release_public_api_id, r.title AS release_title,
       t.created_at
  FROM tracks t
  JOIN releases r ON r.id = t.release_id
 WHERE (NULL::text IS NULL OR t.title ILIKE NULL ESCAPE '\')
   AND (NULL::uuid IS NULL OR r.public_api_id = NULL)
 ORDER BY t.duration_ms DESC NULLS LAST, t.id
 LIMIT 50 OFFSET 0;

\echo '=== T3. catalog tracks: no filter + createdAt:desc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT t.public_api_id, t.title, t.display_artist, t.ordinal,
       t.disc_number, t.track_number, t.duration_ms,
       r.public_api_id AS release_public_api_id, r.title AS release_title,
       t.created_at
  FROM tracks t
  JOIN releases r ON r.id = t.release_id
 WHERE (NULL::text IS NULL OR t.title ILIKE NULL ESCAPE '\')
   AND (NULL::uuid IS NULL OR r.public_api_id = NULL)
 ORDER BY t.created_at DESC, t.id
 LIMIT 50 OFFSET 0;

\echo '=== T4. catalog tracks: ILIKE filter + title:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT t.public_api_id, t.title, t.display_artist, t.ordinal,
       t.disc_number, t.track_number, t.duration_ms,
       r.public_api_id AS release_public_api_id, r.title AS release_title,
       t.created_at
  FROM tracks t
  JOIN releases r ON r.id = t.release_id
 WHERE ('%track 5000%'::text IS NULL OR t.title ILIKE '%track 5000%' ESCAPE '\')
   AND (NULL::uuid IS NULL OR r.public_api_id = NULL)
 ORDER BY t.sort_title ASC, t.id
 LIMIT 50 OFFSET 0;

\echo '=== T5. catalog tracks: releaseId filter + title:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT t.public_api_id, t.title, t.display_artist, t.ordinal,
       t.disc_number, t.track_number, t.duration_ms,
       r.public_api_id AS release_public_api_id, r.title AS release_title,
       t.created_at
  FROM tracks t
  JOIN releases r ON r.id = t.release_id
 WHERE (NULL::text IS NULL OR t.title ILIKE NULL ESCAPE '\')
   AND ((SELECT public_api_id FROM releases ORDER BY id OFFSET 7 LIMIT 1)::uuid IS NULL
        OR r.public_api_id = (SELECT public_api_id FROM releases ORDER BY id OFFSET 7 LIMIT 1))
 ORDER BY t.sort_title ASC, t.id
 LIMIT 50 OFFSET 0;

\echo '=== I1. identity unresolved queue: NOT EXISTS + createdAt:asc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT o.id, o.release_id, r.public_api_id AS release_public_id,
       r.title AS release_title, o.track_id,
       t.public_api_id AS track_public_id, t.title AS track_title,
       o.scope, o.source_ordinal, o.raw_value, o.normalized_value,
       o.display_value, o.source_pointer,
       (SELECT count(DISTINCT proposed.artist_id)
          FROM artist_match_decisions proposed
         WHERE proposed.observed_artist_id = o.id
           AND proposed.outcome = 'proposed') AS pending_match_count,
       (SELECT count(*)
          FROM artist_match_evidence evidence
         WHERE evidence.observed_artist_id = o.id) AS evidence_count,
       o.created_at
  FROM observed_artists o
  JOIN releases r ON r.id = o.release_id
  LEFT JOIN tracks t ON t.id = o.track_id
 WHERE NOT EXISTS (
     SELECT 1 FROM artist_match_current current
     WHERE current.observed_artist_id = o.id)
 ORDER BY o.created_at ASC, o.id
 LIMIT 50 OFFSET 0;

\echo '=== I2. identity unresolved queue: NOT EXISTS + displayValue:desc'
EXPLAIN (ANALYZE, BUFFERS)
SELECT o.id, o.release_id, r.public_api_id AS release_public_id,
       r.title AS release_title, o.track_id,
       t.public_api_id AS track_public_id, t.title AS track_title,
       o.scope, o.source_ordinal, o.raw_value, o.normalized_value,
       o.display_value, o.source_pointer,
       (SELECT count(DISTINCT proposed.artist_id)
          FROM artist_match_decisions proposed
         WHERE proposed.observed_artist_id = o.id
           AND proposed.outcome = 'proposed') AS pending_match_count,
       (SELECT count(*)
          FROM artist_match_evidence evidence
         WHERE evidence.observed_artist_id = o.id) AS evidence_count,
       o.created_at
  FROM observed_artists o
  JOIN releases r ON r.id = o.release_id
  LEFT JOIN tracks t ON t.id = o.track_id
 WHERE NOT EXISTS (
     SELECT 1 FROM artist_match_current current
     WHERE current.observed_artist_id = o.id)
 ORDER BY o.display_value DESC, o.id
 LIMIT 50 OFFSET 0;

\echo '=== I3. identity unresolved count'
EXPLAIN (ANALYZE, BUFFERS)
SELECT COUNT(*)::bigint FROM observed_artists o
WHERE NOT EXISTS (
    SELECT 1 FROM artist_match_current current
    WHERE current.observed_artist_id = o.id);

\echo '=== I4. identity decisions feed: entity_type IN + created_at DESC'
EXPLAIN (ANALYZE, BUFFERS)
SELECT event_id, entity_type, entity_id, event_type, actor,
       reason, payload, created_at
  FROM catalog_audit_events
 WHERE entity_type IN ('observed_artist', 'artist')
 ORDER BY created_at DESC, id DESC
 LIMIT 50 OFFSET 0;

\echo '=== G1. groups listing (tiny table, seq scan acceptable)'
EXPLAIN (ANALYZE, BUFFERS)
SELECT g.id,g.public_api_id,g.name,
       (SELECT count(*) FROM native_user_library_groups m WHERE m.group_id=g.id) AS member_count,
       (SELECT count(*) FROM native_library_group_libraries b WHERE b.group_id=g.id) AS binding_count,
       g.created_at,g.updated_at
  FROM native_library_access_groups g ORDER BY g.name,g.id;
