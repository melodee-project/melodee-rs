-- admin browse index remediation (ADR-0142; PRD-API-ADMIN-002,
-- SPEC §23), closing the index gaps measured in
-- legacy C# behavior audit
--
-- The native browse indexes (migration 0018) are partial
-- (status-filtered) because native v1 never lists merged/deprecated rows. The
-- admin listings deliberately include every status, so these indexes are FULL
-- (non-partial). The partial indexes stay: they are strictly smaller for the
-- hot non-merged native paths and the planner still prefers them when their
-- predicate holds.
--
-- Every btree mirrors its store's exact `ORDER BY …, id` tiebreaker fragment
-- (catalog_browse.rs, identity_review.rs, work_items.rs). The two nullable
-- sort columns (releases.release_date, tracks.duration_ms) are declared
-- `DESC NULLS LAST` to match the stores' `NULLS LAST` ordering: a plain btree
-- cannot serve the measured DESC NULLS LAST direction (verified by EXPLAIN at
-- synthetic catalog scale), and DESC is the operator-common direction
-- (newest/longest first). The ASC NULLS LAST variants fall back to the
-- previous LIMIT-bounded top-N sort — documented in ADR-0142.
--
-- Additive and safe for binary downgrade: no table rewrites, no constraint or
-- identity changes. Rollback is the documented DROP INDEX set in ADR-0142.

CREATE INDEX artists_admin_browse_name_idx
    ON artists (sort_name, id);
CREATE INDEX artists_admin_browse_created_idx
    ON artists (created_at, id);
CREATE INDEX artists_admin_browse_updated_idx
    ON artists (updated_at, id);

CREATE INDEX releases_admin_browse_title_idx
    ON releases (sort_title, id);
CREATE INDEX releases_admin_browse_date_idx
    ON releases (release_date DESC NULLS LAST, id);
CREATE INDEX releases_admin_browse_created_idx
    ON releases (created_at, id);
CREATE INDEX releases_admin_browse_updated_idx
    ON releases (updated_at, id);

CREATE INDEX tracks_admin_browse_duration_idx
    ON tracks (duration_ms DESC NULLS LAST, id);

-- Identity unresolved queue sorts (the identity-review hot path).
CREATE INDEX observed_artists_admin_queue_created_idx
    ON observed_artists (created_at, id);
CREATE INDEX observed_artists_admin_queue_display_idx
    ON observed_artists (display_value, id);

-- Default work-item/review-queue sort (addedAt), also deep paging.
CREATE INDEX release_work_items_admin_browse_created_idx
    ON release_work_items (created_at, id);

-- Identity queue per-row pending-match count subplan
-- (`OBSERVATION_SELECT` in identity_review.rs filters on
-- observed_artist_id + outcome = 'proposed'). The (id, observed_artist_id)
-- UNIQUE index serves this only as a ~4 ms/row skip-scan; measured dominant
-- at page size 50 (report §4 item 2 "also consider", confirmed by the
-- ADR-0142 before/after EXPLAIN: 242 ms → ~2 ms for the queue page).
CREATE INDEX artist_match_decisions_observed_outcome_idx
    ON artist_match_decisions (observed_artist_id, outcome);

-- Full trigram GIN for the admin release title ILIKE filter, mirroring
-- migration 0018's expression minus the partial WHERE clause. The artist
-- listing's OR'd (canonical_name, sort_name, alias EXISTS) ILIKE is NOT
-- bitmap-OR-able, so full artist trigram GINs would never be selected by the
-- planner for that query shape (verified by EXPLAIN); remediating the artist
-- ILIKE path requires the query-flattening rewrite recorded as follow-up work
-- in ADR-0142, not an index.
CREATE INDEX releases_admin_search_title_idx
    ON releases USING gin (title gin_trgm_ops);
