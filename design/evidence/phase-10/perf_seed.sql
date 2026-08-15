-- Phase 10 W4-5 performance evidence: synthetic catalog-scale dataset.
-- Applied ONLY to the throwaway scratch DB `melodee_perf_scratch` on the dev
-- Postgres (port 5433). No migration files are touched; this is a measurement
-- harness, not schema. See performance-report.md for the full procedure.
-- Row-number mapping tables avoid OFFSET/LIMIT lateral scans (O(n^2)).
\set QUIET on
\timing on

-- 200k durable work items across the state machine, spread over ~2 years.
INSERT INTO release_work_items (
    id, api_key, source_path, current_path, state, attempt_count,
    next_attempt_at, correlation_id, discovered_at, updated_at, created_at,
    completed_at, last_error_code, last_error_message)
SELECT uuidv7(), uuidv7(),
       '/inbound/release-' || g, '/work/release-' || g,
       (ARRAY['discovered','settling','ready','inspecting','normalizing',
              'validating','needs_review','staged','publishing','published',
              'cataloging','cataloged','indexing','indexed','failed',
              'quarantined','ignored','deleted'])[1 + (g % 18)],
       g % 4,
       CASE WHEN g % 18 = 14 THEN now() + (g % 72 || ' hours')::interval END,
       uuidv7(),
       now() - (g % 700 || ' days')::interval,
       now() - (g % 700 || ' days')::interval + (g % 40 || ' hours')::interval,
       now() - (g % 700 || ' days')::interval,
       CASE WHEN (g % 18) IN (13, 16, 17) THEN now() - (g % 30 || ' days')::interval END,
       CASE WHEN g % 18 = 14 THEN 'normalize_failed' END,
       CASE WHEN g % 18 = 14 THEN 'ffmpeg exited 1' END
FROM generate_series(1, 200000) g;

-- 100k artists (~3% merged/deprecated so the partial native browse indexes
-- cannot serve the unfiltered admin listing). A dedicated merge target is
-- inserted first so merged rows satisfy artists_merge_check at insert time.
INSERT INTO artists (id, public_api_id, canonical_name, normalized_name,
                     sort_name, artist_type, status, created_at, updated_at)
VALUES ('01999999-9999-7000-8000-000000000001'::uuid, uuidv7(),
        'Merge Target', 'merge target', 'Merge Target', 'group', 'verified',
        now(), now());

INSERT INTO artists (id, public_api_id, canonical_name, normalized_name,
                     sort_name, artist_type, status, merged_into_artist_id,
                     created_at, updated_at)
SELECT uuidv7(), uuidv7(),
       'Artist ' || g, 'artist ' || g,
       CASE WHEN g % 5 = 0 THEN 'The Artist ' || g ELSE 'Artist ' || g END,
       (ARRAY['person','group','orchestra','ensemble','choir','character','other','unknown'])[1 + g % 8],
       CASE WHEN g % 97 = 0 THEN 'merged'
            WHEN g % 89 = 0 THEN 'deprecated'
            WHEN g % 3 = 0 THEN 'provisional' ELSE 'verified' END,
       CASE WHEN g % 97 = 0 THEN '01999999-9999-7000-8000-000000000001'::uuid END,
       now() - (g % 700 || ' days')::interval,
       now() - (g % 100 || ' days')::interval
FROM generate_series(1, 100000) g;

CREATE TEMP TABLE artist_map AS
SELECT row_number() OVER (ORDER BY id) rn, id FROM artists;
CREATE INDEX ON artist_map (rn);

-- 5k aliases to exercise the EXISTS alias filter.
INSERT INTO artist_aliases (id, artist_id, alias, normalized_alias, alias_type,
                            evidence_source, confidence, is_locked, created_at)
SELECT uuidv7(), m.id, 'Alias ' || g, 'alias ' || g, 'search',
       'synthetic', 5000, false, now()
FROM generate_series(1, 5000) g
JOIN artist_map m ON m.rn = 1 + (g % 100000);

-- 100k releases.
INSERT INTO releases (id, public_api_id, title, normalized_title, sort_title,
                      display_artist, release_date, release_date_text, status,
                      created_at, updated_at)
SELECT uuidv7(), uuidv7(),
       'Release ' || g, 'release ' || g,
       CASE WHEN g % 7 = 0 THEN 'A Release ' || g ELSE 'Release ' || g END,
       'Artist ' || (g % 1000),
       CASE WHEN g % 11 <> 0 THEN (date '1960-01-01' + (g % 24000)) END,
       (g % 100 + 1925)::text,
       CASE WHEN g % 61 = 0 THEN 'deprecated' ELSE 'active' END,
       now() - (g % 700 || ' days')::interval,
       now() - (g % 100 || ' days')::interval
FROM generate_series(1, 100000) g;

CREATE TEMP TABLE release_map AS
SELECT row_number() OVER (ORDER BY id) rn, id FROM releases;
CREATE INDEX ON release_map (rn);

-- One billed artist per release (for the artistId EXISTS filter).
INSERT INTO release_artists (id, release_id, artist_id, role, presentation_order,
                             display_value, join_phrase, created_at)
SELECT uuidv7(), r.id, m.id, 'primary', 1, 'Artist', '', now()
FROM release_map r
JOIN artist_map m ON m.rn = 1 + (r.rn % 100000);

-- 500k audio identities + 500k tracks (5 per release).
INSERT INTO audio_identities (id, status, revision, created_at, updated_at)
SELECT uuidv7(), 'verified', 1, now(), now() FROM generate_series(1, 500000) g;

CREATE TEMP TABLE identity_map AS
SELECT row_number() OVER (ORDER BY id) rn, id FROM audio_identities;
CREATE INDEX ON identity_map (rn);

-- One media asset per track; the deferred circular FK
-- (tracks_exactly_one_playable_asset_fk) requires the tracks insert and the
-- playable links inside one transaction.
CREATE TEMP TABLE asset_map AS
SELECT row_number() OVER () rn, uuidv7() AS id
FROM generate_series(1, 500000);
CREATE INDEX ON asset_map (rn);

INSERT INTO media_assets (id, canonical_relative_path, byte_hash, byte_size,
                          mime_type, container, codec, duration_ms, created_at)
SELECT id, 'library/synthetic/' || id::text || '.flac',
       md5(rn::text) || md5(rn::text), 25000000 + rn,
       'audio/flac', 'flac', 'flac', 240000, now()
FROM asset_map;

BEGIN;

INSERT INTO tracks (id, public_api_id, release_id, audio_identity_id,
                    playable_media_asset_id, ordinal, title, normalized_title,
                    sort_title, display_artist, disc_number, track_number,
                    duration_ms, source_path, created_at, updated_at)
SELECT uuidv7(), uuidv7(), r.id, ai.id, am.id,
       t.ord,
       'Track ' || r.rn * 10 + t.ord, 'track ' || r.rn * 10 + t.ord,
       'Track ' || r.rn * 10 + t.ord,
       CASE WHEN t.ord = 1 THEN 'Artist ' || (r.rn % 1000) END,
       1, t.ord,
       60000 + ((r.rn * 7 + t.ord) % 600000),
       '/library/r' || r.rn || '/t' || t.ord || '.flac',
       now() - (r.rn % 700 || ' days')::interval, now()
FROM release_map r
CROSS JOIN generate_series(1, 5) t(ord)
JOIN identity_map ai ON ai.rn = (r.rn - 1) * 5 + t.ord
JOIN asset_map am ON am.rn = (r.rn - 1) * 5 + t.ord;

INSERT INTO track_media_assets (track_id, media_asset_id, purpose, ordinal,
                                created_at)
SELECT t.id, t.playable_media_asset_id, 'playable', 1, now() FROM tracks t;

COMMIT;

-- Catalog aggregate projections (1 row per artist/release).
INSERT INTO artist_catalog_aggregates (artist_id, release_count, billed_track_count,
                                       performance_track_count, all_credit_track_count,
                                       revision, updated_at)
SELECT id, 1, 5, 0, 5, 1, now() FROM artists;
INSERT INTO release_catalog_aggregates (release_id, track_count, duration_ms,
                                        media_bytes, revision, updated_at)
SELECT id, 5, 1200000, 250000000, 1, now() FROM releases;

-- 200k observed artists (2 per release); 80% unresolved.
INSERT INTO observed_artists (id, release_id, track_id, scope, source_ordinal,
                              raw_value, normalized_value, display_value,
                              source_pointer, source_payload, created_at)
SELECT uuidv7(), r.id, NULL, 'release', s.ord,
       'Observed ' || r.rn || '-' || s.ord, 'observed ' || r.rn || '-' || s.ord,
       'Observed ' || r.rn || '-' || s.ord,
       'tag:artist:' || s.ord, '{}', now() - (r.rn % 700 || ' days')::interval
FROM release_map r
CROSS JOIN generate_series(1, 2) s(ord);

CREATE TEMP TABLE observation_map AS
SELECT row_number() OVER (ORDER BY o.id) rn, o.id
FROM observed_artists o WHERE o.source_ordinal = 1;
CREATE INDEX ON observation_map (rn);

-- Resolve 20%: authoritative decision + current pointer.
INSERT INTO artist_match_decisions (id, observed_artist_id, artist_id, outcome,
                                    authority, is_locked, confidence, reason,
                                    payload, created_at)
SELECT uuidv7(), o.id, m.id, 'confirmed', 'automated', false, 9000,
       'synthetic seed', '{}', now()
FROM observation_map o
JOIN artist_map m ON m.rn = 1 + (o.rn % 100000)
WHERE o.rn % 5 = 0;

INSERT INTO artist_match_current (observed_artist_id, decision_id, updated_at)
SELECT observed_artist_id, id, now() FROM artist_match_decisions;

-- Proposed candidates + evidence on unresolved rows (drives the queue counts).
INSERT INTO artist_match_decisions (id, observed_artist_id, artist_id, outcome,
                                    authority, is_locked, confidence, reason,
                                    payload, created_at)
SELECT uuidv7(), o.id, m.id, 'proposed', 'automated', false, 6000,
       'synthetic candidate', '{}', now()
FROM (SELECT o.id, row_number() OVER (ORDER BY o.id) rn FROM observed_artists o
      WHERE NOT EXISTS (SELECT 1 FROM artist_match_current c
                        WHERE c.observed_artist_id = o.id)) o
JOIN artist_map m ON m.rn = 1 + (o.rn % 100000);

INSERT INTO artist_match_evidence (id, observed_artist_id, candidate_artist_id,
                                   evidence_kind, polarity, confidence, source,
                                   payload, created_at)
SELECT uuidv7(), d.observed_artist_id, d.artist_id, 'normalized_name_exact',
       'supports', 6000, 'synthetic', '{}', now()
FROM artist_match_decisions d;

-- 300k audit events across entity types (identity feed filter target).
INSERT INTO catalog_audit_events (event_id, entity_type, entity_id, event_type,
                                  actor, reason, payload, created_at)
SELECT uuidv7(),
       (ARRAY['observed_artist','artist','release','track'])[1 + g % 4],
       uuidv7(), (ARRAY['match_decision','merge','split','edit'])[1 + g % 4],
       'synthetic', 'seed', '{}', now() - (g % 700 || ' days')::interval
FROM generate_series(1, 300000) g;

-- 5k users (user administration is not catalog-scale; completeness only).
INSERT INTO native_users (id, public_api_id, username, normalized_username,
                          email, normalized_email, password_hash,
                          password_hash_algorithm, stream_signing_key,
                          is_admin, is_editor, enabled, locked, blacklisted,
                          auth_revision, created_at, updated_at)
SELECT uuidv7(), uuidv7(), 'user' || g, 'user' || g,
       'user' || g || '@example.com', 'user' || g || '@example.com',
       '$2b$12$synthetichashvalueforperf0000000000000000000000000000', 'bcrypt',
       'stream-key-' || g, g = 1, false, true, false, false, 1,
       now() - (g % 700 || ' days')::interval, now()
FROM generate_series(1, 5000) g;

ANALYZE release_work_items;
ANALYZE artists;
ANALYZE artist_aliases;
ANALYZE releases;
ANALYZE release_artists;
ANALYZE audio_identities;
ANALYZE tracks;
ANALYZE artist_catalog_aggregates;
ANALYZE release_catalog_aggregates;
ANALYZE observed_artists;
ANALYZE artist_match_decisions;
ANALYZE artist_match_current;
ANALYZE artist_match_evidence;
ANALYZE catalog_audit_events;
ANALYZE native_users;
