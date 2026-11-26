-- PERF remediation: predicate-aligned indexes and verified duplicate removal.
--
-- Forward-only and additive except for three schema-proven exact duplicate
-- indexes. Every statement here is ordinary transactional DDL executed under
-- the ADR-0166 maintenance fence (all Melodee writers stopped, one migration
-- invocation, readiness/integrity verification before reopening traffic), so
-- no CONCURRENTLY variant is required or valid inside the SQLx transaction.
--
-- Rollback: the three dropped indexes are recreated by the identical CREATE
-- INDEX statements below in a verified pre-migration restore, or with a new
-- forward migration. No column, constraint, or row semantics change.

-- PERF-001 / RM-01: the media filename trigger reverse-looks-up every Track
-- that plays an inserted or path-updated asset. Without a leading
-- playable_media_asset_id index that lookup is a catalog-wide sequential scan
-- on every media asset insert. Narrow key only: the trigger reads track.id,
-- so no INCLUDE columns are justified without a plan that proves an
-- index-only benefit.
CREATE INDEX tracks_playable_media_asset_idx
    ON tracks (playable_media_asset_id);

-- PERF-003 / RM-03: release search hydration and native/OpenSubsonic genre
-- reads filter catalog_genre_terms by release_id and surface. Existing
-- indexes lead with surface or track_id, so this is the missing leading
-- release access path.
CREATE INDEX catalog_genre_terms_release_surface_idx
    ON catalog_genre_terms (release_id, surface, display_value);

-- PERF-004 / RM-09: reconciliation candidate lookup filters decisions by
-- artist_id. Existing indexes lead with observed_artist_id or outcome.
CREATE INDEX artist_match_decisions_artist_observed_idx
    ON artist_match_decisions (artist_id, observed_artist_id);

-- PERF-011 / RM-16: drop only indexes whose complete definitions are exact
-- duplicates of an existing unique constraint index. Verified by
-- pg_get_indexdef equivalence and by the constraint dependency review in
-- PERF-REMEDIATION-RESULTS.md. `IF EXISTS` keeps a partially upgraded or
-- hand-repaired database converging; these are never migration-owned final
-- objects under a different name.
--
-- tracks_native_random_candidate_idx (id) == tracks_pkey (id)
DROP INDEX IF EXISTS tracks_native_random_candidate_idx;
-- artist_track_associations_native_track_idx (track_id, artist_id)
--   == artist_track_associations_track_idx (track_id, artist_id)
DROP INDEX IF EXISTS artist_track_associations_native_track_idx;
-- search_outbox_hourly_totals_age_idx (projection_name, hour)
--   == search_outbox_hourly_totals_pkey (projection_name, hour)
DROP INDEX IF EXISTS search_outbox_hourly_totals_age_idx;

-- PERF-010 / RM-15: OpenSubsonic getArtists/getIndexes computes a correlated
-- AVG(rating) over native_artist_user_states filtered to `rating > 0` and
-- excluding the current user. The existing lists index is keyed
-- `(user_id, starred, ...)`, so the aggregate walked per-artist rows through a
-- shape that does not match its predicate. A partial leading artist index
-- answers the aggregate directly and stays small because zero-rating rows are
-- excluded.
CREATE INDEX native_artist_user_states_positive_rating_idx
    ON native_artist_user_states (artist_id)
    INCLUDE (user_id, rating)
    WHERE rating > 0;
