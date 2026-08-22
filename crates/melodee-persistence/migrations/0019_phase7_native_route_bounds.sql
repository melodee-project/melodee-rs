-- bounded native playlist import and recommendation projections
-- (ADR-0123, ADR-0128; PRD-PLAYLIST-001, PRD-DISCOVERY-002..003).
--
-- Additive only. Binary downgrade leaves these rebuildable projections and
-- their maintenance triggers in place. Full schema rollback follows the
-- verified pre-migration restore-and-forward-migrate procedure. Neither
-- projection owns catalog identity or media and neither deletes media.

CREATE TABLE native_track_file_names (
    track_id uuid PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    normalized_file_name text NOT NULL,
    CONSTRAINT native_track_file_names_value CHECK (
        btrim(normalized_file_name) <> ''
        AND char_length(normalized_file_name) <= 4096
    )
);

CREATE INDEX native_track_file_names_lookup_idx
    ON native_track_file_names (normalized_file_name, track_id);

CREATE FUNCTION refresh_native_track_file_name(p_track_id uuid) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    file_name text;
BEGIN
    SELECT lower(regexp_replace(
        COALESCE(track.source_path, media.canonical_relative_path), '^.*/', ''))
    INTO file_name
    FROM tracks track
    JOIN media_assets media ON media.id = track.playable_media_asset_id
    WHERE track.id = p_track_id;

    IF file_name IS NULL OR btrim(file_name) = '' OR char_length(file_name) > 4096 THEN
        DELETE FROM native_track_file_names WHERE track_id = p_track_id;
    ELSE
        INSERT INTO native_track_file_names (track_id, normalized_file_name)
        VALUES (p_track_id, file_name)
        ON CONFLICT (track_id) DO UPDATE
        SET normalized_file_name = excluded.normalized_file_name;
    END IF;
END;
$$;

CREATE FUNCTION refresh_native_track_file_name_from_track() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM refresh_native_track_file_name(NEW.id);
    RETURN NULL;
END;
$$;

CREATE FUNCTION refresh_native_track_file_name_from_media() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM refresh_native_track_file_name(track.id)
    FROM tracks track
    WHERE track.playable_media_asset_id = NEW.id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER tracks_native_file_name_refresh
AFTER INSERT OR UPDATE OF source_path, playable_media_asset_id ON tracks
FOR EACH ROW EXECUTE FUNCTION refresh_native_track_file_name_from_track();

CREATE TRIGGER media_assets_native_file_name_refresh
AFTER INSERT OR UPDATE OF canonical_relative_path ON media_assets
FOR EACH ROW EXECUTE FUNCTION refresh_native_track_file_name_from_media();

INSERT INTO native_track_file_names (track_id, normalized_file_name)
SELECT track.id, lower(regexp_replace(
    COALESCE(track.source_path, media.canonical_relative_path), '^.*/', ''))
FROM tracks track
JOIN media_assets media ON media.id = track.playable_media_asset_id
WHERE btrim(COALESCE(track.source_path, media.canonical_relative_path)) <> ''
  AND char_length(lower(regexp_replace(
      COALESCE(track.source_path, media.canonical_relative_path), '^.*/', ''))) <= 4096;

CREATE TABLE native_recommendation_candidates (
    entity_type text NOT NULL,
    entity_id uuid NOT NULL,
    popularity bigint NOT NULL DEFAULT 0,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (entity_type, entity_id),
    CONSTRAINT native_recommendation_candidates_type CHECK (
        entity_type IN ('track', 'release', 'artist')
    ),
    CONSTRAINT native_recommendation_candidates_popularity CHECK (popularity >= 0)
);

CREATE INDEX native_recommendation_candidates_rank_idx
    ON native_recommendation_candidates
       (entity_type, popularity DESC, entity_id);

-- Recommendation category probes bind the principal and candidate Track,
-- then apply the recent-listening cutoff. The request window bounds
-- candidate count; this composite index also bounds work inside each probe.
CREATE INDEX native_track_plays_user_track_time_idx
    ON native_track_plays (user_id, track_id, played_at DESC);

INSERT INTO native_recommendation_candidates (entity_type, entity_id, popularity, updated_at)
SELECT 'track', track.id, count(play.id), now()
FROM tracks track
LEFT JOIN native_track_plays play ON play.track_id = track.id
GROUP BY track.id
UNION ALL
SELECT 'release', release.id, count(play.id), now()
FROM releases release
LEFT JOIN tracks track ON track.release_id = release.id
LEFT JOIN native_track_plays play ON play.track_id = track.id
GROUP BY release.id
UNION ALL
SELECT 'artist', artist.id, count(play.id), now()
FROM artists artist
LEFT JOIN track_artists billing ON billing.artist_id = artist.id AND billing.role = 'primary'
LEFT JOIN native_track_plays play ON play.track_id = billing.track_id
GROUP BY artist.id;

CREATE FUNCTION sync_native_recommendation_catalog_candidate() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    kind text := TG_ARGV[0];
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM native_recommendation_candidates
        WHERE entity_type = kind AND entity_id = OLD.id;
    ELSE
        INSERT INTO native_recommendation_candidates (
            entity_type, entity_id, popularity, updated_at
        ) VALUES (kind, NEW.id, 0, now())
        ON CONFLICT (entity_type, entity_id) DO NOTHING;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER tracks_native_recommendation_candidate
AFTER INSERT OR DELETE ON tracks
FOR EACH ROW EXECUTE FUNCTION sync_native_recommendation_catalog_candidate('track');
CREATE TRIGGER releases_native_recommendation_candidate
AFTER INSERT OR DELETE ON releases
FOR EACH ROW EXECUTE FUNCTION sync_native_recommendation_catalog_candidate('release');
CREATE TRIGGER artists_native_recommendation_candidate
AFTER INSERT OR DELETE ON artists
FOR EACH ROW EXECUTE FUNCTION sync_native_recommendation_catalog_candidate('artist');

CREATE FUNCTION adjust_native_recommendation_candidates(
    p_track_id uuid,
    p_delta bigint
) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    release_id uuid;
BEGIN
    SELECT track.release_id INTO release_id FROM tracks track WHERE track.id = p_track_id;
    IF release_id IS NULL THEN
        RETURN;
    END IF;

    UPDATE native_recommendation_candidates
    SET popularity = GREATEST(0, popularity + p_delta), updated_at = now()
    WHERE entity_type = 'track' AND entity_id = p_track_id;
    UPDATE native_recommendation_candidates
    SET popularity = GREATEST(0, popularity + p_delta), updated_at = now()
    WHERE entity_type = 'release' AND entity_id = release_id;
    UPDATE native_recommendation_candidates candidate
    SET popularity = GREATEST(0, candidate.popularity + p_delta), updated_at = now()
    FROM track_artists billing
    WHERE candidate.entity_type = 'artist'
      AND candidate.entity_id = billing.artist_id
      AND billing.track_id = p_track_id
      AND billing.role = 'primary';
END;
$$;

-- Relationship corrections and play ingestion must observe one serialized
-- Track relationship. Row locks avoid the snapshot race where a play could
-- increment the old Release/Artist after a concurrent reassignment had
-- already counted the play set for the new relationship. Multi-Track updates
-- lock in UUID order so opposing Track moves cannot deadlock.
CREATE FUNCTION lock_native_recommendation_tracks(p_track_ids uuid[]) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM track.id
    FROM tracks track
    WHERE track.id = ANY(p_track_ids)
    ORDER BY track.id
    FOR UPDATE;
END;
$$;

CREATE FUNCTION maintain_native_recommendation_popularity() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM lock_native_recommendation_tracks(ARRAY[OLD.track_id]);
    ELSIF TG_OP = 'INSERT' THEN
        PERFORM lock_native_recommendation_tracks(ARRAY[NEW.track_id]);
    ELSIF OLD.track_id IS DISTINCT FROM NEW.track_id THEN
        PERFORM lock_native_recommendation_tracks(ARRAY[OLD.track_id, NEW.track_id]);
    ELSE
        PERFORM lock_native_recommendation_tracks(ARRAY[NEW.track_id]);
    END IF;
    IF TG_OP = 'DELETE' OR (TG_OP = 'UPDATE' AND OLD.track_id <> NEW.track_id) THEN
        PERFORM adjust_native_recommendation_candidates(OLD.track_id, -1);
    END IF;
    IF TG_OP = 'INSERT' OR (TG_OP = 'UPDATE' AND OLD.track_id <> NEW.track_id) THEN
        PERFORM adjust_native_recommendation_candidates(NEW.track_id, 1);
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER native_track_plays_recommendation_popularity
AFTER INSERT OR DELETE OR UPDATE OF track_id ON native_track_plays
FOR EACH ROW EXECUTE FUNCTION maintain_native_recommendation_popularity();

-- Popularity follows catalog relationships as they are corrected. Without
-- these maintenance paths a Track reassigned to another Release or primary
-- Artist would leave the old entity ranked and the new entity under-counted
-- until a full projection rebuild.
CREATE FUNCTION maintain_native_recommendation_track_release() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    track_popularity bigint;
BEGIN
    PERFORM lock_native_recommendation_tracks(ARRAY[NEW.id]);
    SELECT count(*) INTO track_popularity
    FROM native_track_plays play WHERE play.track_id = NEW.id;

    UPDATE native_recommendation_candidates
    SET popularity = GREATEST(0, popularity - track_popularity), updated_at = now()
    WHERE entity_type = 'release' AND entity_id = OLD.release_id;
    UPDATE native_recommendation_candidates
    SET popularity = popularity + track_popularity, updated_at = now()
    WHERE entity_type = 'release' AND entity_id = NEW.release_id;
    RETURN NULL;
END;
$$;

CREATE TRIGGER tracks_native_recommendation_release_refresh
AFTER UPDATE OF release_id ON tracks
FOR EACH ROW
WHEN (OLD.release_id IS DISTINCT FROM NEW.release_id)
EXECUTE FUNCTION maintain_native_recommendation_track_release();

CREATE FUNCTION maintain_native_recommendation_primary_artist() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    track_popularity bigint;
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM lock_native_recommendation_tracks(ARRAY[OLD.track_id]);
    ELSIF TG_OP = 'INSERT' THEN
        PERFORM lock_native_recommendation_tracks(ARRAY[NEW.track_id]);
    ELSE
        PERFORM lock_native_recommendation_tracks(ARRAY[OLD.track_id, NEW.track_id]);
    END IF;
    IF TG_OP IN ('DELETE', 'UPDATE') AND OLD.role = 'primary' THEN
        SELECT count(*) INTO track_popularity
        FROM native_track_plays play WHERE play.track_id = OLD.track_id;
        UPDATE native_recommendation_candidates
        SET popularity = GREATEST(0, popularity - track_popularity), updated_at = now()
        WHERE entity_type = 'artist' AND entity_id = OLD.artist_id;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') AND NEW.role = 'primary' THEN
        SELECT count(*) INTO track_popularity
        FROM native_track_plays play WHERE play.track_id = NEW.track_id;
        UPDATE native_recommendation_candidates
        SET popularity = popularity + track_popularity, updated_at = now()
        WHERE entity_type = 'artist' AND entity_id = NEW.artist_id;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER track_artists_native_recommendation_refresh
AFTER INSERT OR DELETE OR UPDATE OF track_id, artist_id, role ON track_artists
FOR EACH ROW EXECUTE FUNCTION maintain_native_recommendation_primary_artist();
