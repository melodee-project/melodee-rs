-- native user annotations, playlists, and playback history
-- (ADR-0114, ADR-0127; PRD-USER-001..004, PRD-PLAYLIST-001..003).
--
-- Additive only. Binary downgrade retains these tables. Full schema rollback
-- restores a verified pre-0011 backup into a new database. No statement reads,
-- moves, or deletes media.

CREATE TABLE native_artist_user_states (
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    starred boolean NOT NULL DEFAULT false,
    hated boolean NOT NULL DEFAULT false,
    rating smallint NOT NULL DEFAULT 0,
    pinned boolean NOT NULL DEFAULT false,
    play_count bigint NOT NULL DEFAULT 0,
    last_played_at timestamptz NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (user_id, artist_id),
    CONSTRAINT native_artist_user_states_rating_check CHECK (rating BETWEEN 0 AND 5),
    CONSTRAINT native_artist_user_states_play_count_check CHECK (play_count >= 0),
    CONSTRAINT native_artist_user_states_time_check CHECK (created_at <= updated_at)
);

CREATE INDEX native_artist_user_states_lists_idx
    ON native_artist_user_states (user_id, starred, hated, rating, updated_at DESC, artist_id);

CREATE TABLE native_release_user_states (
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    starred boolean NOT NULL DEFAULT false,
    hated boolean NOT NULL DEFAULT false,
    rating smallint NOT NULL DEFAULT 0,
    pinned boolean NOT NULL DEFAULT false,
    play_count bigint NOT NULL DEFAULT 0,
    last_played_at timestamptz NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (user_id, release_id),
    CONSTRAINT native_release_user_states_rating_check CHECK (rating BETWEEN 0 AND 5),
    CONSTRAINT native_release_user_states_play_count_check CHECK (play_count >= 0),
    CONSTRAINT native_release_user_states_time_check CHECK (created_at <= updated_at)
);

CREATE INDEX native_release_user_states_lists_idx
    ON native_release_user_states (user_id, starred, hated, rating, updated_at DESC, release_id);

CREATE TABLE native_track_user_states (
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    starred boolean NOT NULL DEFAULT false,
    hated boolean NOT NULL DEFAULT false,
    rating smallint NOT NULL DEFAULT 0,
    pinned boolean NOT NULL DEFAULT false,
    play_count bigint NOT NULL DEFAULT 0,
    last_played_at timestamptz NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (user_id, track_id),
    CONSTRAINT native_track_user_states_rating_check CHECK (rating BETWEEN 0 AND 5),
    CONSTRAINT native_track_user_states_play_count_check CHECK (play_count >= 0),
    CONSTRAINT native_track_user_states_time_check CHECK (created_at <= updated_at)
);

CREATE INDEX native_track_user_states_lists_idx
    ON native_track_user_states (user_id, starred, hated, rating, updated_at DESC, track_id);
CREATE INDEX native_track_user_states_recent_idx
    ON native_track_user_states (user_id, last_played_at DESC, track_id)
    WHERE last_played_at IS NOT NULL;

CREATE TABLE native_playlists (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    owner_user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    name text NOT NULL,
    description text NULL,
    is_public boolean NOT NULL DEFAULT false,
    revision bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_playlists_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT native_playlists_name_check CHECK (btrim(name) <> '' AND char_length(name) <= 500),
    CONSTRAINT native_playlists_description_check CHECK (
        description IS NULL OR char_length(description) <= 10000
    ),
    CONSTRAINT native_playlists_revision_check CHECK (revision > 0),
    CONSTRAINT native_playlists_time_check CHECK (created_at <= updated_at)
);

CREATE INDEX native_playlists_owner_idx
    ON native_playlists (owner_user_id, created_at DESC, public_api_id);
CREATE INDEX native_playlists_public_idx
    ON native_playlists (created_at DESC, public_api_id) WHERE is_public;

CREATE TRIGGER native_playlists_public_api_id_immutable
BEFORE UPDATE OF public_api_id ON native_playlists
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();

CREATE TABLE native_playlist_items (
    id uuid PRIMARY KEY,
    playlist_id uuid NOT NULL REFERENCES native_playlists (id) ON DELETE CASCADE,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    position integer NOT NULL,
    added_by_user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL,
    CONSTRAINT native_playlist_items_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT native_playlist_items_position_check CHECK (position >= 0),
    CONSTRAINT native_playlist_items_position_unique
        UNIQUE (playlist_id, position) DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX native_playlist_items_track_idx
    ON native_playlist_items (track_id, playlist_id, position);

CREATE TABLE native_track_plays (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    client_event_id text NOT NULL,
    played_at timestamptz NOT NULL,
    submitted_at timestamptz NOT NULL,
    duration_seconds integer NULL,
    completed boolean NOT NULL DEFAULT true,
    CONSTRAINT native_track_plays_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT native_track_plays_event_check CHECK (
        btrim(client_event_id) <> '' AND char_length(client_event_id) <= 256
    ),
    CONSTRAINT native_track_plays_duration_check CHECK (
        duration_seconds IS NULL OR duration_seconds >= 0
    ),
    CONSTRAINT native_track_plays_time_check CHECK (played_at <= submitted_at),
    UNIQUE (user_id, client_event_id)
);

CREATE INDEX native_track_plays_user_time_idx
    ON native_track_plays (user_id, played_at DESC, id);
CREATE INDEX native_track_plays_track_time_idx
    ON native_track_plays (track_id, played_at DESC, id);

CREATE TABLE native_now_playing (
    user_id uuid PRIMARY KEY REFERENCES native_users (id) ON DELETE RESTRICT,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    client_event_id text NOT NULL,
    started_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_now_playing_event_check CHECK (
        btrim(client_event_id) <> '' AND char_length(client_event_id) <= 256
    ),
    CONSTRAINT native_now_playing_time_check CHECK (started_at <= updated_at)
);
