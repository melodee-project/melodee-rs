-- PERF-003/PERF-004 remediation correctness: concurrent genre aggregate
-- maintenance and an idempotent repair operation.
--
-- The previous statement-level triggers recomputed each affected aggregate
-- from the *statement snapshot* and then overwrote the stored row. Two
-- concurrent transactions that each added one Track on a new genre could both
-- count one and both store one, losing a contribution permanently. Sorting or
-- row locks alone do not fix that: a later transaction must also observe the
-- earlier committed contribution.
--
-- Protocol implemented here (single protocol for every mutation path):
--   1. Collect the affected (surface, display_value) keys from the transition
--      table (insert, delete) or from Releases whose status actually changed.
--   2. Acquire transaction-scoped advisory locks for those keys in ascending
--      computed lock-key order, so concurrent import/delete/status/repair
--      transactions cannot deadlock.
--   3. Recount from authoritative catalog_genre_terms joined to active
--      Releases. Each SPI statement in a VOLATILE function runs with a fresh
--      READ COMMITTED snapshot, so a transaction that waited for the advisory
--      lock observes the contribution committed by the transaction it waited
--      for. Lock order proves progress; the fresh snapshot proves visibility.
--   4. Remove keys that now have zero active contributors so no stale
--      aggregate survives a real transition to zero.
--
-- The reconstruction below keeps every statement-level transition-table
-- trigger (the rejected `UPDATE OF status ... REFERENCING` form is invalid in
-- PostgreSQL) and adds an internal actual-status-change guard.

CREATE OR REPLACE FUNCTION refresh_catalog_genre_aggregate_keys(
    p_surfaces text[],
    p_display_values text[]
) RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    lock_key bigint;
BEGIN
    IF p_surfaces IS NULL
       OR p_display_values IS NULL
       OR array_length(p_surfaces, 1) IS NULL
       OR array_length(p_surfaces, 1) <> array_length(p_display_values, 1)
    THEN
        RETURN;
    END IF;

    -- Deterministic actual lock-key order, not input order and not lexical
    -- name order. Every caller uses this helper, so the order is global.
    FOR lock_key IN
        SELECT DISTINCT hashtextextended(affected.surface || E'\x1f' || affected.display_value, 9301)
        FROM unnest(p_surfaces, p_display_values) AS affected(surface, display_value)
        ORDER BY 1
    LOOP
        PERFORM pg_advisory_xact_lock(lock_key);
    END LOOP;

    INSERT INTO catalog_genre_aggregates (
        surface, display_value, song_count, release_count
    )
    SELECT term.surface, term.display_value, count(*)::bigint,
           count(DISTINCT term.release_id)::bigint
    FROM catalog_genre_terms term
    JOIN releases release ON release.id = term.release_id
        AND release.status = 'active'
    JOIN (
        SELECT DISTINCT surface, display_value
        FROM unnest(p_surfaces, p_display_values) AS affected(surface, display_value)
    ) keys USING (surface, display_value)
    GROUP BY term.surface, term.display_value
    ON CONFLICT (surface, display_value) DO UPDATE
    SET song_count = EXCLUDED.song_count,
        release_count = EXCLUDED.release_count;

    DELETE FROM catalog_genre_aggregates aggregate
    USING (
        SELECT DISTINCT surface, display_value
        FROM unnest(p_surfaces, p_display_values) AS affected(surface, display_value)
    ) keys
    WHERE aggregate.surface = keys.surface
      AND aggregate.display_value = keys.display_value
      AND NOT EXISTS (
          SELECT 1
          FROM catalog_genre_terms remaining
          JOIN releases release ON release.id = remaining.release_id
              AND release.status = 'active'
          WHERE remaining.surface = keys.surface
            AND remaining.display_value = keys.display_value
      );
END;
$$;

CREATE OR REPLACE FUNCTION refresh_inserted_catalog_genre_aggregates()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    affected_surfaces text[];
    affected_displays text[];
BEGIN
    SELECT array_agg(surface ORDER BY surface, display_value),
           array_agg(display_value ORDER BY surface, display_value)
    INTO affected_surfaces, affected_displays
    FROM (SELECT DISTINCT surface, display_value FROM inserted_genre_terms) affected;

    PERFORM refresh_catalog_genre_aggregate_keys(affected_surfaces, affected_displays);
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION refresh_deleted_catalog_genre_aggregates()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    affected_surfaces text[];
    affected_displays text[];
BEGIN
    SELECT array_agg(surface ORDER BY surface, display_value),
           array_agg(display_value ORDER BY surface, display_value)
    INTO affected_surfaces, affected_displays
    FROM (SELECT DISTINCT surface, display_value FROM deleted_genre_terms) affected;

    PERFORM refresh_catalog_genre_aggregate_keys(affected_surfaces, affected_displays);
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION refresh_release_status_catalog_genre_aggregates()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    affected_surfaces text[];
    affected_displays text[];
BEGIN
    -- Overlay-only Release updates (title, cover, metadata pointers, revision)
    -- must perform no aggregate work at all.
    IF NOT EXISTS (
        SELECT 1
        FROM old_release_rows previous
        JOIN new_release_rows current ON current.id = previous.id
        WHERE current.status IS DISTINCT FROM previous.status
    ) THEN
        RETURN NULL;
    END IF;

    SELECT array_agg(term.surface ORDER BY term.surface, term.display_value),
           array_agg(term.display_value ORDER BY term.surface, term.display_value)
    INTO affected_surfaces, affected_displays
    FROM (
        SELECT DISTINCT genre.surface, genre.display_value
        FROM catalog_genre_terms genre
        JOIN (
            SELECT previous.id
            FROM old_release_rows previous
            JOIN new_release_rows current ON current.id = previous.id
            WHERE current.status IS DISTINCT FROM previous.status
        ) changed ON changed.id = genre.release_id
    ) term;

    PERFORM refresh_catalog_genre_aggregate_keys(affected_surfaces, affected_displays);
    RETURN NULL;
END;
$$;

-- ---------------------------------------------------------------------------
-- Idempotent repair for aggregates already corrupted by the old protocol.
-- ---------------------------------------------------------------------------

-- Read-only discrepancy report bounded by a caller-supplied limit. A row is a
-- discrepancy when the stored song/release counts differ from the counts
-- derived from authoritative term rows and active Release status, including a
-- stored row that no longer has any contributor and a missing stored row.
CREATE OR REPLACE FUNCTION catalog_genre_aggregate_discrepancies(p_limit integer)
RETURNS TABLE (
    surface text,
    display_value text,
    stored_song_count bigint,
    stored_release_count bigint,
    actual_song_count bigint,
    actual_release_count bigint
)
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(stored.surface, derived.surface) AS surface,
           COALESCE(stored.display_value, derived.display_value) AS display_value,
           stored.song_count AS stored_song_count,
           stored.release_count AS stored_release_count,
           COALESCE(derived.song_count, 0) AS actual_song_count,
           COALESCE(derived.release_count, 0) AS actual_release_count
    FROM catalog_genre_aggregates stored
    FULL OUTER JOIN (
        SELECT term.surface, term.display_value,
               count(*)::bigint AS song_count,
               count(DISTINCT term.release_id)::bigint AS release_count
        FROM catalog_genre_terms term
        JOIN releases release ON release.id = term.release_id
            AND release.status = 'active'
        GROUP BY term.surface, term.display_value
    ) derived ON derived.surface = stored.surface
             AND derived.display_value = stored.display_value
    WHERE stored.song_count IS DISTINCT FROM derived.song_count
       OR stored.release_count IS DISTINCT FROM derived.release_count
    ORDER BY 1, 2
    LIMIT p_limit;
$$;

-- Bounded, resumable repair. `p_apply = false` only reports; callers loop
-- until `checked` returns zero. `p_apply = true` acquires the same advisory
-- lock protocol as live maintenance, deletes zero-contributor rows, and
-- upserts the authoritative counts. The function is idempotent: a second
-- invocation over repaired rows reports nothing to do, and a crash between
-- batches leaves earlier batches correct and later batches still pending.
CREATE OR REPLACE FUNCTION repair_catalog_genre_aggregates(
    p_apply boolean,
    p_limit integer
) RETURNS TABLE (checked integer, repaired integer)
LANGUAGE plpgsql
AS $$
DECLARE
    lock_key bigint;
    checked_count integer;
    repaired_count integer;
BEGIN
    IF p_limit IS NULL OR p_limit < 1 THEN
        RAISE EXCEPTION 'genre aggregate repair limit must be positive';
    END IF;

    CREATE TEMP TABLE IF NOT EXISTS perf_genre_repair_keys (
        surface text NOT NULL,
        display_value text NOT NULL,
        PRIMARY KEY (surface, display_value)
    ) ON COMMIT DROP;
    TRUNCATE perf_genre_repair_keys;

    INSERT INTO perf_genre_repair_keys (surface, display_value)
    SELECT discrepancy.surface, discrepancy.display_value
    FROM catalog_genre_aggregate_discrepancies(p_limit) discrepancy;

    SELECT count(*)::integer INTO checked_count FROM perf_genre_repair_keys;
    repaired_count := 0;
    IF checked_count = 0 THEN
        RETURN QUERY SELECT 0, 0;
        RETURN;
    END IF;

    FOR lock_key IN
        SELECT DISTINCT hashtextextended(keys.surface || E'\x1f' || keys.display_value, 9301)
        FROM perf_genre_repair_keys keys
        ORDER BY 1
    LOOP
        PERFORM pg_advisory_xact_lock(lock_key);
    END LOOP;

    IF p_apply THEN
        DELETE FROM catalog_genre_aggregates aggregate
        USING perf_genre_repair_keys keys
        WHERE aggregate.surface = keys.surface
          AND aggregate.display_value = keys.display_value
          AND NOT EXISTS (
              SELECT 1
              FROM catalog_genre_terms remaining
              JOIN releases release ON release.id = remaining.release_id
                  AND release.status = 'active'
              WHERE remaining.surface = keys.surface
                AND remaining.display_value = keys.display_value
          );

        INSERT INTO catalog_genre_aggregates (
            surface, display_value, song_count, release_count
        )
        SELECT term.surface, term.display_value, count(*)::bigint,
               count(DISTINCT term.release_id)::bigint
        FROM catalog_genre_terms term
        JOIN releases release ON release.id = term.release_id
            AND release.status = 'active'
        JOIN perf_genre_repair_keys keys
          ON keys.surface = term.surface
         AND keys.display_value = term.display_value
        GROUP BY term.surface, term.display_value
        ON CONFLICT (surface, display_value) DO UPDATE
        SET song_count = EXCLUDED.song_count,
            release_count = EXCLUDED.release_count;

        repaired_count := checked_count;
    END IF;

    RETURN QUERY SELECT checked_count, repaired_count;
END;
$$;

-- ---------------------------------------------------------------------------
-- Keep the serving projection consistent when a Track changes Release.
-- ---------------------------------------------------------------------------
--
-- Merge and split move Tracks between Releases with a set-based UPDATE. The
-- genre term rows denormalize the owning Release id, so without this trigger
-- the moved Track's terms kept the merged-away Release and the surviving
-- Release's aggregate lost those contributors. A statement-level transition
-- table keeps this one statement per moved Track batch (no per-Track round
-- trips) and requeues only the keys that actually moved. The `release_id`
-- change is detected inside the function because a column list cannot be
-- combined with transition tables in PostgreSQL.
CREATE OR REPLACE FUNCTION maintain_catalog_genre_terms_on_track_move()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    affected_surfaces text[];
    affected_displays text[];
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM old_track_rows previous
        JOIN new_track_rows current ON current.id = previous.id
        WHERE current.release_id IS DISTINCT FROM previous.release_id
    ) THEN
        RETURN NULL;
    END IF;

    WITH moved AS (
        UPDATE catalog_genre_terms term
        SET release_id = shift.release_id
        FROM (
            SELECT current.id, current.release_id
            FROM old_track_rows previous
            JOIN new_track_rows current ON current.id = previous.id
            WHERE current.release_id IS DISTINCT FROM previous.release_id
        ) shift
        WHERE term.track_id = shift.id
        RETURNING term.surface, term.display_value
    )
    SELECT array_agg(surface ORDER BY surface, display_value),
           array_agg(display_value ORDER BY surface, display_value)
    INTO affected_surfaces, affected_displays
    FROM (SELECT DISTINCT surface, display_value FROM moved) keys;

    PERFORM refresh_catalog_genre_aggregate_keys(affected_surfaces, affected_displays);
    RETURN NULL;
END;
$$;

CREATE TRIGGER tracks_catalog_genre_terms_refresh
AFTER UPDATE ON tracks
REFERENCING OLD TABLE AS old_track_rows NEW TABLE AS new_track_rows
FOR EACH STATEMENT
EXECUTE FUNCTION maintain_catalog_genre_terms_on_track_move();
