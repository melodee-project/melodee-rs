-- ADR-0165: narrow, indexed genre serving facts replace request-time
-- expansion of immutable manifest JSON. The source manifest and PostgreSQL
-- catalog remain authoritative; these rows are a rebuildable projection.
--
-- Forward/rollback: additive tables, functions, and triggers only. A binary
-- downgrade leaves them inert. Destructive rollback restores a verified
-- pre-0038 backup rather than dropping serving evidence in place.

-- Fence catalog publication while the one-time backfill and trigger install
-- share one transaction. ADR-0166 requires every Melodee writer to be stopped
-- for this migration, so this lock is a fail-closed backstop, not an online-
-- migration claim.
LOCK TABLE catalog_imports IN SHARE ROW EXCLUSIVE MODE;

CREATE TABLE catalog_genre_terms (
    track_id uuid NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    release_id uuid NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    surface text NOT NULL,
    display_value text NOT NULL,
    source_order bigint NOT NULL,
    match_value text GENERATED ALWAYS AS (btrim(display_value)) STORED,
    normalized_value text GENERATED ALWAYS AS (lower(btrim(display_value))) STORED,
    PRIMARY KEY (track_id, surface, display_value),
    CONSTRAINT catalog_genre_terms_surface_check
        CHECK (surface IN ('native', 'opensubsonic')),
    CONSTRAINT catalog_genre_terms_value_check
        CHECK (btrim(display_value) <> '')
);

CREATE INDEX catalog_genre_terms_release_match_idx
    ON catalog_genre_terms (surface, match_value, release_id, track_id);
CREATE INDEX catalog_genre_terms_track_match_idx
    ON catalog_genre_terms (track_id, surface, match_value);
CREATE INDEX catalog_genre_terms_normalized_idx
    ON catalog_genre_terms (surface, normalized_value, track_id);

CREATE TABLE catalog_genre_aggregates (
    surface text NOT NULL,
    display_value text NOT NULL,
    match_value text GENERATED ALWAYS AS (btrim(display_value)) STORED,
    normalized_value text GENERATED ALWAYS AS (lower(btrim(display_value))) STORED,
    song_count bigint NOT NULL,
    release_count bigint NOT NULL,
    PRIMARY KEY (surface, display_value),
    CONSTRAINT catalog_genre_aggregates_surface_check
        CHECK (surface IN ('native', 'opensubsonic')),
    CONSTRAINT catalog_genre_aggregates_counts_check
        CHECK (song_count > 0 AND release_count > 0 AND release_count <= song_count)
);

CREATE INDEX catalog_genre_aggregates_order_idx
    ON catalog_genre_aggregates (surface, display_value COLLATE "C");
CREATE INDEX catalog_genre_aggregates_normalized_idx
    ON catalog_genre_aggregates (surface, normalized_value, display_value);

-- Native v1 preserves one trimmed value per Track.
INSERT INTO catalog_genre_terms (
    track_id, release_id, surface, display_value, source_order
)
SELECT DISTINCT ON (track.id, btrim(file.value->'metadata'->>'genre'))
       track.id, track.release_id, 'native',
       btrim(file.value->'metadata'->>'genre'), file.ordinality
FROM catalog_imports catalog_import
JOIN tracks track ON track.release_id = catalog_import.release_id
JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id
CROSS JOIN LATERAL jsonb_array_elements(
    COALESCE(manifest.manifest_payload->'files', '[]'))
    WITH ORDINALITY file(value, ordinality)
WHERE file.value->>'relative_path' = track.source_path
  AND btrim(COALESCE(file.value->'metadata'->>'genre', '')) <> ''
ORDER BY track.id, btrim(file.value->'metadata'->>'genre'), file.ordinality;

-- OpenSubsonic preserves the exact semicolon-split display term while
-- matching on its trimmed value.
INSERT INTO catalog_genre_terms (
    track_id, release_id, surface, display_value, source_order
)
SELECT DISTINCT ON (track.id, genre_part.value)
       track.id, track.release_id, 'opensubsonic', genre_part.value,
       file.ordinality * 100000 + genre_part.ordinality
FROM catalog_imports catalog_import
JOIN tracks track ON track.release_id = catalog_import.release_id
JOIN release_work_item_manifests manifest ON manifest.id = catalog_import.manifest_row_id
CROSS JOIN LATERAL jsonb_array_elements(
    COALESCE(manifest.manifest_payload->'files', '[]'))
    WITH ORDINALITY file(value, ordinality)
CROSS JOIN LATERAL unnest(string_to_array(
    file.value->'metadata'->>'genre', ';'))
    WITH ORDINALITY genre_part(value, ordinality)
WHERE file.value->>'relative_path' = track.source_path
  AND btrim(genre_part.value) <> ''
ORDER BY track.id, genre_part.value,
         file.ordinality * 100000 + genre_part.ordinality;

INSERT INTO catalog_genre_aggregates (
    surface, display_value, song_count, release_count
)
SELECT surface, display_value, count(*)::bigint,
       count(DISTINCT release_id)::bigint
FROM catalog_genre_terms
JOIN releases release ON release.id = catalog_genre_terms.release_id
    AND release.status = 'active'
GROUP BY surface, display_value;

CREATE FUNCTION refresh_inserted_catalog_genre_aggregates()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO catalog_genre_aggregates (
        surface, display_value, song_count, release_count
    )
    SELECT term.surface, term.display_value, count(*)::bigint,
           count(DISTINCT term.release_id)::bigint
    FROM catalog_genre_terms term
    JOIN releases release ON release.id = term.release_id
        AND release.status = 'active'
    JOIN (
        SELECT DISTINCT surface, display_value FROM inserted_genre_terms
    ) affected USING (surface, display_value)
    GROUP BY term.surface, term.display_value
    ON CONFLICT (surface, display_value) DO UPDATE
    SET song_count = EXCLUDED.song_count,
        release_count = EXCLUDED.release_count;
    RETURN NULL;
END;
$$;

CREATE FUNCTION refresh_deleted_catalog_genre_aggregates()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO catalog_genre_aggregates (
        surface, display_value, song_count, release_count
    )
    SELECT term.surface, term.display_value, count(*)::bigint,
           count(DISTINCT term.release_id)::bigint
    FROM catalog_genre_terms term
    JOIN releases release ON release.id = term.release_id
        AND release.status = 'active'
    JOIN (
        SELECT DISTINCT surface, display_value FROM deleted_genre_terms
    ) affected USING (surface, display_value)
    GROUP BY term.surface, term.display_value
    ON CONFLICT (surface, display_value) DO UPDATE
    SET song_count = EXCLUDED.song_count,
        release_count = EXCLUDED.release_count;

    DELETE FROM catalog_genre_aggregates aggregate
    USING (
        SELECT DISTINCT surface, display_value FROM deleted_genre_terms
    ) affected
    WHERE aggregate.surface = affected.surface
      AND aggregate.display_value = affected.display_value
      AND NOT EXISTS (
          SELECT 1
          FROM catalog_genre_terms remaining
          JOIN releases release ON release.id = remaining.release_id
              AND release.status = 'active'
          WHERE remaining.surface = affected.surface
            AND remaining.display_value = affected.display_value
      );
    RETURN NULL;
END;
$$;

-- Aggregate membership follows Release serving status. Catalog maintenance
-- can merge, deprecate, or restore a Release without touching its immutable
-- term rows, so status changes must refresh only that Release's genre keys.
CREATE FUNCTION refresh_release_status_catalog_genre_aggregates()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO catalog_genre_aggregates (
        surface, display_value, song_count, release_count
    )
    SELECT term.surface, term.display_value, count(*)::bigint,
           count(DISTINCT term.release_id)::bigint
    FROM catalog_genre_terms term
    JOIN releases release ON release.id = term.release_id
        AND release.status = 'active'
    JOIN (
        SELECT DISTINCT affected_term.surface, affected_term.display_value
        FROM catalog_genre_terms affected_term
        JOIN (
            SELECT id FROM new_release_rows
            UNION
            SELECT id FROM old_release_rows
        ) affected_release ON affected_release.id = affected_term.release_id
    ) affected USING (surface, display_value)
    GROUP BY term.surface, term.display_value
    ON CONFLICT (surface, display_value) DO UPDATE
    SET song_count = EXCLUDED.song_count,
        release_count = EXCLUDED.release_count;

    DELETE FROM catalog_genre_aggregates aggregate
    USING (
        SELECT DISTINCT affected_term.surface, affected_term.display_value
        FROM catalog_genre_terms affected_term
        JOIN (
            SELECT id FROM new_release_rows
            UNION
            SELECT id FROM old_release_rows
        ) affected_release ON affected_release.id = affected_term.release_id
    ) affected
    WHERE aggregate.surface = affected.surface
      AND aggregate.display_value = affected.display_value
      AND NOT EXISTS (
          SELECT 1
          FROM catalog_genre_terms remaining
          JOIN releases release ON release.id = remaining.release_id
              AND release.status = 'active'
          WHERE remaining.surface = affected.surface
            AND remaining.display_value = affected.display_value
      );
    RETURN NULL;
END;
$$;

CREATE TRIGGER catalog_genre_terms_insert_aggregates
AFTER INSERT ON catalog_genre_terms
REFERENCING NEW TABLE AS inserted_genre_terms
FOR EACH STATEMENT
EXECUTE FUNCTION refresh_inserted_catalog_genre_aggregates();

CREATE TRIGGER catalog_genre_terms_delete_aggregates
AFTER DELETE ON catalog_genre_terms
REFERENCING OLD TABLE AS deleted_genre_terms
FOR EACH STATEMENT
EXECUTE FUNCTION refresh_deleted_catalog_genre_aggregates();

CREATE TRIGGER release_status_catalog_genre_aggregates
AFTER UPDATE ON releases
REFERENCING OLD TABLE AS old_release_rows NEW TABLE AS new_release_rows
FOR EACH STATEMENT
EXECUTE FUNCTION refresh_release_status_catalog_genre_aggregates();

CREATE FUNCTION project_catalog_import_genres()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO catalog_genre_terms (
        track_id, release_id, surface, display_value, source_order
    )
    SELECT DISTINCT ON (track.id, btrim(file.value->'metadata'->>'genre'))
           track.id, track.release_id, 'native',
           btrim(file.value->'metadata'->>'genre'), file.ordinality
    FROM tracks track
    JOIN release_work_item_manifests manifest ON manifest.id = NEW.manifest_row_id
    CROSS JOIN LATERAL jsonb_array_elements(
        COALESCE(manifest.manifest_payload->'files', '[]'))
        WITH ORDINALITY file(value, ordinality)
    WHERE track.release_id = NEW.release_id
      AND file.value->>'relative_path' = track.source_path
      AND btrim(COALESCE(file.value->'metadata'->>'genre', '')) <> ''
    ORDER BY track.id, btrim(file.value->'metadata'->>'genre'), file.ordinality
    ON CONFLICT DO NOTHING;

    INSERT INTO catalog_genre_terms (
        track_id, release_id, surface, display_value, source_order
    )
    SELECT DISTINCT ON (track.id, genre_part.value)
           track.id, track.release_id, 'opensubsonic', genre_part.value,
           file.ordinality * 100000 + genre_part.ordinality
    FROM tracks track
    JOIN release_work_item_manifests manifest ON manifest.id = NEW.manifest_row_id
    CROSS JOIN LATERAL jsonb_array_elements(
        COALESCE(manifest.manifest_payload->'files', '[]'))
        WITH ORDINALITY file(value, ordinality)
    CROSS JOIN LATERAL unnest(string_to_array(
        file.value->'metadata'->>'genre', ';'))
        WITH ORDINALITY genre_part(value, ordinality)
    WHERE track.release_id = NEW.release_id
      AND file.value->>'relative_path' = track.source_path
      AND btrim(genre_part.value) <> ''
    ORDER BY track.id, genre_part.value,
             file.ordinality * 100000 + genre_part.ordinality
    ON CONFLICT DO NOTHING;
    RETURN NEW;
END;
$$;

CREATE TRIGGER catalog_import_genre_projection
AFTER INSERT ON catalog_imports
FOR EACH ROW
EXECUTE FUNCTION project_catalog_import_genres();
