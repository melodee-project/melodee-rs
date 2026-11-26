-- Forward-only under ADR-0166: stop all writers before upgrading. This replaces
-- row-level popularity maintenance with statement-level grouped deltas. Foreign
-- key checks take KEY SHARE on Tracks; NO KEY UPDATE is compatible with those
-- checks while still serializing relationship changes and plays. All affected
-- Track and candidate locks are acquired in deterministic order before writes.
-- For duplicate primary credits, Artist popularity follows the discovery recount
-- (one contribution per credit); the old play trigger counted an Artist once.
-- Existing drift converges through bounded discovery maintenance.
-- Roll back by restoring the verified pre-upgrade backup; do not edit history.

-- Supports two forward-only, ordered cursor scans per scrobble without sorting
-- the whole Artist-credit set in an application buffer. The partial predicate
-- matches the exact primary-credit lookup.
CREATE INDEX track_artists_primary_artist_order_idx
    ON track_artists (track_id, artist_id) WHERE role='primary';

CREATE OR REPLACE FUNCTION lock_native_recommendation_tracks(p_track_ids uuid[])
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM track.id FROM tracks track
    WHERE track.id = ANY(p_track_ids) ORDER BY track.id FOR NO KEY UPDATE;
END;
$$;

DROP TRIGGER native_track_plays_recommendation_popularity ON native_track_plays;
DROP TRIGGER tracks_native_recommendation_release_refresh ON tracks;
DROP TRIGGER track_artists_native_recommendation_refresh ON track_artists;

CREATE FUNCTION sync_recommendation_plays_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM new_rows) THEN RETURN NULL; END IF;
    -- All Track rows precede all recommendation rows in the shared protocol.
    PERFORM t.id FROM tracks t JOIN (SELECT DISTINCT track_id FROM (SELECT track_id,count(*)::bigint delta FROM new_rows GROUP BY track_id) delta WHERE delta<>0) affected ON affected.track_id=t.id
    ORDER BY t.id FOR NO KEY UPDATE OF t;
    PERFORM c.entity_id FROM native_recommendation_candidates c
    JOIN (SELECT 'track'::text entity_type,d.track_id entity_id FROM (SELECT track_id,count(*)::bigint delta FROM new_rows GROUP BY track_id) d WHERE d.delta<>0
        UNION SELECT 'release',t.release_id FROM (SELECT track_id,count(*)::bigint delta FROM new_rows GROUP BY track_id) d JOIN tracks t ON t.id=d.track_id WHERE d.delta<>0
        UNION SELECT 'artist',a.artist_id FROM (SELECT track_id,count(*)::bigint delta FROM new_rows GROUP BY track_id) d JOIN track_artists a ON a.track_id=d.track_id AND a.role='primary' WHERE d.delta<>0) affected ON affected.entity_type=c.entity_type AND affected.entity_id=c.entity_id
    ORDER BY c.entity_type,c.entity_id FOR UPDATE OF c;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT track_id,count(*)::bigint delta FROM new_rows GROUP BY track_id) d WHERE c.entity_type='track' AND c.entity_id=d.track_id AND d.delta<>0;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT t.release_id id,sum(x.delta)::bigint delta FROM (SELECT track_id,count(*)::bigint delta FROM new_rows GROUP BY track_id) x JOIN tracks t ON t.id=x.track_id GROUP BY t.release_id) d
    WHERE c.entity_type='release' AND c.entity_id=d.id AND d.delta<>0;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT a.artist_id id,sum(x.delta)::bigint delta FROM (SELECT track_id,count(*)::bigint delta FROM new_rows GROUP BY track_id) x
          JOIN LATERAL (SELECT artist_id FROM track_artists WHERE track_id=x.track_id AND role='primary') a ON true GROUP BY a.artist_id) d
    WHERE c.entity_type='artist' AND c.entity_id=d.id AND d.delta<>0;
    RETURN NULL;
END;
$$;
CREATE TRIGGER sync_recommendation_plays_insert_trigger AFTER INSERT ON native_track_plays
REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION sync_recommendation_plays_insert();

CREATE FUNCTION sync_recommendation_plays_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM old_rows) THEN RETURN NULL; END IF;
    -- All Track rows precede all recommendation rows in the shared protocol.
    PERFORM t.id FROM tracks t JOIN (SELECT DISTINCT track_id FROM (SELECT track_id,-count(*)::bigint delta FROM old_rows GROUP BY track_id) delta WHERE delta<>0) affected ON affected.track_id=t.id
    ORDER BY t.id FOR NO KEY UPDATE OF t;
    PERFORM c.entity_id FROM native_recommendation_candidates c
    JOIN (SELECT 'track'::text entity_type,d.track_id entity_id FROM (SELECT track_id,-count(*)::bigint delta FROM old_rows GROUP BY track_id) d WHERE d.delta<>0
        UNION SELECT 'release',t.release_id FROM (SELECT track_id,-count(*)::bigint delta FROM old_rows GROUP BY track_id) d JOIN tracks t ON t.id=d.track_id WHERE d.delta<>0
        UNION SELECT 'artist',a.artist_id FROM (SELECT track_id,-count(*)::bigint delta FROM old_rows GROUP BY track_id) d JOIN track_artists a ON a.track_id=d.track_id AND a.role='primary' WHERE d.delta<>0) affected ON affected.entity_type=c.entity_type AND affected.entity_id=c.entity_id
    ORDER BY c.entity_type,c.entity_id FOR UPDATE OF c;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT track_id,-count(*)::bigint delta FROM old_rows GROUP BY track_id) d WHERE c.entity_type='track' AND c.entity_id=d.track_id AND d.delta<>0;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT t.release_id id,sum(x.delta)::bigint delta FROM (SELECT track_id,-count(*)::bigint delta FROM old_rows GROUP BY track_id) x JOIN tracks t ON t.id=x.track_id GROUP BY t.release_id) d
    WHERE c.entity_type='release' AND c.entity_id=d.id AND d.delta<>0;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT a.artist_id id,sum(x.delta)::bigint delta FROM (SELECT track_id,-count(*)::bigint delta FROM old_rows GROUP BY track_id) x
          JOIN LATERAL (SELECT artist_id FROM track_artists WHERE track_id=x.track_id AND role='primary') a ON true GROUP BY a.artist_id) d
    WHERE c.entity_type='artist' AND c.entity_id=d.id AND d.delta<>0;
    RETURN NULL;
END;
$$;
CREATE TRIGGER sync_recommendation_plays_delete_trigger AFTER DELETE ON native_track_plays
REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION sync_recommendation_plays_delete();

CREATE FUNCTION sync_recommendation_plays_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- All Track rows precede all recommendation rows in the shared protocol.
    PERFORM t.id FROM tracks t JOIN (SELECT DISTINCT track_id FROM (SELECT track_id,sum(delta)::bigint delta FROM (SELECT track_id,1::bigint delta FROM new_rows UNION ALL SELECT track_id,-1::bigint delta FROM old_rows) plays GROUP BY track_id HAVING sum(delta)<>0) delta WHERE delta<>0) affected ON affected.track_id=t.id
    ORDER BY t.id FOR NO KEY UPDATE OF t;
    PERFORM c.entity_id FROM native_recommendation_candidates c
    JOIN (SELECT 'track'::text entity_type,d.track_id entity_id FROM (SELECT track_id,sum(delta)::bigint delta FROM (SELECT track_id,1::bigint delta FROM new_rows UNION ALL SELECT track_id,-1::bigint delta FROM old_rows) plays GROUP BY track_id HAVING sum(delta)<>0) d WHERE d.delta<>0
        UNION SELECT 'release',t.release_id FROM (SELECT track_id,sum(delta)::bigint delta FROM (SELECT track_id,1::bigint delta FROM new_rows UNION ALL SELECT track_id,-1::bigint delta FROM old_rows) plays GROUP BY track_id HAVING sum(delta)<>0) d JOIN tracks t ON t.id=d.track_id WHERE d.delta<>0
        UNION SELECT 'artist',a.artist_id FROM (SELECT track_id,sum(delta)::bigint delta FROM (SELECT track_id,1::bigint delta FROM new_rows UNION ALL SELECT track_id,-1::bigint delta FROM old_rows) plays GROUP BY track_id HAVING sum(delta)<>0) d JOIN track_artists a ON a.track_id=d.track_id AND a.role='primary' WHERE d.delta<>0) affected ON affected.entity_type=c.entity_type AND affected.entity_id=c.entity_id
    ORDER BY c.entity_type,c.entity_id FOR UPDATE OF c;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT track_id,sum(delta)::bigint delta FROM (SELECT track_id,1::bigint delta FROM new_rows UNION ALL SELECT track_id,-1::bigint delta FROM old_rows) plays GROUP BY track_id HAVING sum(delta)<>0) d WHERE c.entity_type='track' AND c.entity_id=d.track_id AND d.delta<>0;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT t.release_id id,sum(x.delta)::bigint delta FROM (SELECT track_id,sum(delta)::bigint delta FROM (SELECT track_id,1::bigint delta FROM new_rows UNION ALL SELECT track_id,-1::bigint delta FROM old_rows) plays GROUP BY track_id HAVING sum(delta)<>0) x JOIN tracks t ON t.id=x.track_id GROUP BY t.release_id) d
    WHERE c.entity_type='release' AND c.entity_id=d.id AND d.delta<>0;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT a.artist_id id,sum(x.delta)::bigint delta FROM (SELECT track_id,sum(delta)::bigint delta FROM (SELECT track_id,1::bigint delta FROM new_rows UNION ALL SELECT track_id,-1::bigint delta FROM old_rows) plays GROUP BY track_id HAVING sum(delta)<>0) x
          JOIN LATERAL (SELECT artist_id FROM track_artists WHERE track_id=x.track_id AND role='primary') a ON true GROUP BY a.artist_id) d
    WHERE c.entity_type='artist' AND c.entity_id=d.id AND d.delta<>0;
    RETURN NULL;
END;
$$;
CREATE TRIGGER sync_recommendation_plays_update_trigger AFTER UPDATE ON native_track_plays
REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION sync_recommendation_plays_update();

CREATE FUNCTION sync_recommendation_credit_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM new_rows) THEN RETURN NULL; END IF;
    -- All Track rows precede all recommendation rows in the shared protocol.
    PERFORM t.id FROM tracks t JOIN (SELECT DISTINCT track_id FROM (SELECT track_id,artist_id,count(*)::bigint delta FROM new_rows WHERE role='primary' GROUP BY track_id,artist_id) delta WHERE delta<>0) affected ON affected.track_id=t.id
    ORDER BY t.id FOR NO KEY UPDATE OF t;
    PERFORM c.entity_id FROM native_recommendation_candidates c
    JOIN (SELECT 'artist'::text entity_type,artist_id entity_id FROM (SELECT track_id,artist_id,count(*)::bigint delta FROM new_rows WHERE role='primary' GROUP BY track_id,artist_id) delta WHERE delta<>0 GROUP BY artist_id) affected ON affected.entity_type=c.entity_type AND affected.entity_id=c.entity_id
    ORDER BY c.entity_type,c.entity_id FOR UPDATE OF c;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT x.artist_id id,sum(x.delta * plays.play_count)::bigint delta FROM (SELECT track_id,artist_id,count(*)::bigint delta FROM new_rows WHERE role='primary' GROUP BY track_id,artist_id) x
          JOIN LATERAL (SELECT count(*)::bigint play_count FROM native_track_plays p WHERE p.track_id=x.track_id) plays ON true
          GROUP BY x.artist_id) d WHERE c.entity_type='artist' AND c.entity_id=d.id AND d.delta<>0;
    RETURN NULL;
END;
$$;
CREATE TRIGGER sync_recommendation_credit_insert_trigger AFTER INSERT ON track_artists
REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION sync_recommendation_credit_insert();

CREATE FUNCTION sync_recommendation_credit_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM old_rows) THEN RETURN NULL; END IF;
    -- All Track rows precede all recommendation rows in the shared protocol.
    PERFORM t.id FROM tracks t JOIN (SELECT DISTINCT track_id FROM (SELECT track_id,artist_id,-count(*)::bigint delta FROM old_rows WHERE role='primary' GROUP BY track_id,artist_id) delta WHERE delta<>0) affected ON affected.track_id=t.id
    ORDER BY t.id FOR NO KEY UPDATE OF t;
    PERFORM c.entity_id FROM native_recommendation_candidates c
    JOIN (SELECT 'artist'::text entity_type,artist_id entity_id FROM (SELECT track_id,artist_id,-count(*)::bigint delta FROM old_rows WHERE role='primary' GROUP BY track_id,artist_id) delta WHERE delta<>0 GROUP BY artist_id) affected ON affected.entity_type=c.entity_type AND affected.entity_id=c.entity_id
    ORDER BY c.entity_type,c.entity_id FOR UPDATE OF c;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT x.artist_id id,sum(x.delta * plays.play_count)::bigint delta FROM (SELECT track_id,artist_id,-count(*)::bigint delta FROM old_rows WHERE role='primary' GROUP BY track_id,artist_id) x
          JOIN LATERAL (SELECT count(*)::bigint play_count FROM native_track_plays p WHERE p.track_id=x.track_id) plays ON true
          GROUP BY x.artist_id) d WHERE c.entity_type='artist' AND c.entity_id=d.id AND d.delta<>0;
    RETURN NULL;
END;
$$;
CREATE TRIGGER sync_recommendation_credit_delete_trigger AFTER DELETE ON track_artists
REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION sync_recommendation_credit_delete();

CREATE FUNCTION sync_recommendation_credit_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- All Track rows precede all recommendation rows in the shared protocol.
    PERFORM t.id FROM tracks t JOIN (SELECT DISTINCT track_id FROM (SELECT track_id,artist_id,sum(delta)::bigint delta FROM (SELECT track_id,artist_id,1::bigint delta FROM new_rows WHERE role='primary' UNION ALL SELECT track_id,artist_id,-1::bigint delta FROM old_rows WHERE role='primary') credits GROUP BY track_id,artist_id HAVING sum(delta)<>0) delta WHERE delta<>0) affected ON affected.track_id=t.id
    ORDER BY t.id FOR NO KEY UPDATE OF t;
    PERFORM c.entity_id FROM native_recommendation_candidates c
    JOIN (SELECT 'artist'::text entity_type,artist_id entity_id FROM (SELECT track_id,artist_id,sum(delta)::bigint delta FROM (SELECT track_id,artist_id,1::bigint delta FROM new_rows WHERE role='primary' UNION ALL SELECT track_id,artist_id,-1::bigint delta FROM old_rows WHERE role='primary') credits GROUP BY track_id,artist_id HAVING sum(delta)<>0) delta WHERE delta<>0 GROUP BY artist_id) affected ON affected.entity_type=c.entity_type AND affected.entity_id=c.entity_id
    ORDER BY c.entity_type,c.entity_id FOR UPDATE OF c;
    UPDATE native_recommendation_candidates c SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
    FROM (SELECT x.artist_id id,sum(x.delta * plays.play_count)::bigint delta FROM (SELECT track_id,artist_id,sum(delta)::bigint delta FROM (SELECT track_id,artist_id,1::bigint delta FROM new_rows WHERE role='primary' UNION ALL SELECT track_id,artist_id,-1::bigint delta FROM old_rows WHERE role='primary') credits GROUP BY track_id,artist_id HAVING sum(delta)<>0) x
          JOIN LATERAL (SELECT count(*)::bigint play_count FROM native_track_plays p WHERE p.track_id=x.track_id) plays ON true
          GROUP BY x.artist_id) d WHERE c.entity_type='artist' AND c.entity_id=d.id AND d.delta<>0;
    RETURN NULL;
END;
$$;
CREATE TRIGGER sync_recommendation_credit_update_trigger AFTER UPDATE ON track_artists
REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION sync_recommendation_credit_update();

CREATE FUNCTION sync_recommendation_track_release() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- All Track rows precede all recommendation rows in the shared protocol.
    PERFORM t.id FROM tracks t JOIN (SELECT track_id FROM (SELECT n.id track_id,o.release_id old_release,n.release_id new_release FROM old_rows o JOIN new_rows n USING(id) WHERE o.release_id IS DISTINCT FROM n.release_id) changed) affected ON affected.track_id=t.id
    ORDER BY t.id FOR NO KEY UPDATE OF t;
    PERFORM c.entity_id FROM native_recommendation_candidates c
    JOIN (SELECT 'release'::text entity_type,old_release entity_id FROM (SELECT n.id track_id,o.release_id old_release,n.release_id new_release FROM old_rows o JOIN new_rows n USING(id) WHERE o.release_id IS DISTINCT FROM n.release_id) changed
        UNION SELECT 'release',new_release FROM (SELECT n.id track_id,o.release_id old_release,n.release_id new_release FROM old_rows o JOIN new_rows n USING(id) WHERE o.release_id IS DISTINCT FROM n.release_id) changed) affected ON affected.entity_type=c.entity_type AND affected.entity_id=c.entity_id
    ORDER BY c.entity_type,c.entity_id FOR UPDATE OF c;
    UPDATE native_recommendation_candidates c
       SET popularity=GREATEST(0,c.popularity+d.delta),updated_at=now()
      FROM (
        WITH per_track AS MATERIALIZED (
            SELECT o.release_id old_release,n.release_id new_release,
                   (SELECT count(*)::bigint FROM native_track_plays p WHERE p.track_id=n.id) play_count
            FROM old_rows o JOIN new_rows n USING (id)
            WHERE o.release_id IS DISTINCT FROM n.release_id
        )
        SELECT id,sum(delta)::bigint delta FROM (
            SELECT old_release id,-play_count delta FROM per_track
            UNION ALL
            SELECT new_release id,play_count delta FROM per_track
        ) effects GROUP BY id
      ) d
     WHERE c.entity_type='release' AND c.entity_id=d.id AND d.delta<>0;
    RETURN NULL;
END;
$$;
CREATE TRIGGER sync_recommendation_track_release_trigger AFTER UPDATE ON tracks
REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION sync_recommendation_track_release();
