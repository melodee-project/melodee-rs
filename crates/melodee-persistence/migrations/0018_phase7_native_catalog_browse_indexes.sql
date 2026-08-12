-- Phase 7 native catalog browse paths (ADR-0123; PRD-API-M1-006).
--
-- These additive indexes keep stable native-v1 pagination index-backed at
-- current catalog scale. They do not change authoritative catalog identity or
-- media facts and are safe for binary downgrade.

CREATE INDEX artists_native_browse_name_idx
    ON artists (sort_name, public_api_id)
    WHERE status NOT IN ('merged', 'deprecated');

CREATE INDEX artists_native_browse_created_idx
    ON artists (created_at, public_api_id)
    WHERE status NOT IN ('merged', 'deprecated');

CREATE INDEX releases_native_browse_title_idx
    ON releases (sort_title, public_api_id)
    WHERE status = 'active';

CREATE INDEX releases_native_browse_date_idx
    ON releases (release_date, public_api_id)
    WHERE status = 'active';

CREATE INDEX releases_native_browse_created_idx
    ON releases (created_at, public_api_id)
    WHERE status = 'active';

CREATE INDEX tracks_native_browse_title_idx ON tracks (sort_title, public_api_id);
CREATE INDEX tracks_native_browse_created_idx ON tracks (created_at, public_api_id);
CREATE INDEX tracks_native_random_candidate_idx ON tracks (id);

CREATE INDEX release_artists_native_release_primary_idx
    ON release_artists (release_id, role, presentation_order, id)
    INCLUDE (artist_id);

CREATE INDEX track_artists_native_track_primary_idx
    ON track_artists (track_id, role, presentation_order, id)
    INCLUDE (artist_id);

CREATE INDEX artist_track_associations_native_track_idx
    ON artist_track_associations (track_id, artist_id);

-- Native v1 preserves case-insensitive substring search. PostgreSQL's bundled
-- trigram operator class keeps that exact wire behavior index-backed rather
-- than silently narrowing it to prefix or token matching. The bundled
-- system-rows sampler gives the random route a fixed-size, full-domain sample
-- instead of sorting or permanently truncating the catalog.
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;
CREATE EXTENSION IF NOT EXISTS tsm_system_rows WITH SCHEMA public;

CREATE INDEX artists_native_search_name_idx
    ON artists USING gin (canonical_name gin_trgm_ops)
    WHERE status NOT IN ('merged', 'deprecated');
CREATE INDEX artist_aliases_native_search_alias_idx
    ON artist_aliases USING gin (alias gin_trgm_ops);
CREATE INDEX releases_native_search_title_idx
    ON releases USING gin (title gin_trgm_ops)
    WHERE status = 'active';
CREATE INDEX tracks_native_search_title_idx
    ON tracks USING gin (title gin_trgm_ops);
CREATE INDEX native_playlists_native_search_name_idx
    ON native_playlists USING gin (name gin_trgm_ops);

-- A stable hash key supports an indexed random pivot with circular wraparound.
-- Every Track is reachable without a whole-catalog random sort.
CREATE INDEX tracks_native_random_hash_idx
    ON tracks (hashtextextended(public_api_id::text, 0), public_api_id)
    INCLUDE (id);
