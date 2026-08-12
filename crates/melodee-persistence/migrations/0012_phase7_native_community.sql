-- Phase 7 native requests, shares, and playback preferences
-- (ADR-0123, ADR-0127; PRD-REQUEST-001..002, PRD-SHARE-001..003,
-- PRD-USER-003).
--
-- Additive only. Binary downgrade retains these tables. Full schema rollback
-- restores a verified pre-0012 backup into a new database. Catalog and media
-- rows are referenced, never rewritten or deleted by these tables.

CREATE TABLE native_requests (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    created_by_user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    category smallint NOT NULL,
    status smallint NOT NULL DEFAULT 1,
    description text NOT NULL,
    artist_name text NULL,
    target_artist_public_api_id uuid NULL,
    release_title text NULL,
    target_release_public_api_id uuid NULL,
    track_title text NULL,
    target_track_public_api_id uuid NULL,
    release_year integer NULL,
    external_url text NULL,
    notes text NULL,
    last_activity_at timestamptz NOT NULL,
    last_activity_type text NOT NULL DEFAULT 'UserComment',
    last_activity_user_id uuid NULL REFERENCES native_users (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_requests_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT native_requests_category_check CHECK (category BETWEEN 1 AND 5),
    CONSTRAINT native_requests_status_check CHECK (status BETWEEN 1 AND 4),
    CONSTRAINT native_requests_description_check CHECK (
        btrim(description) <> '' AND char_length(description) <= 10000
    ),
    CONSTRAINT native_requests_names_check CHECK (
        (artist_name IS NULL OR char_length(artist_name) <= 500)
        AND (release_title IS NULL OR char_length(release_title) <= 500)
        AND (track_title IS NULL OR char_length(track_title) <= 500)
    ),
    CONSTRAINT native_requests_external_url_check CHECK (
        external_url IS NULL OR char_length(external_url) <= 4096
    ),
    CONSTRAINT native_requests_notes_check CHECK (notes IS NULL OR char_length(notes) <= 10000),
    CONSTRAINT native_requests_time_check CHECK (
        created_at <= updated_at AND created_at <= last_activity_at
    )
);

CREATE INDEX native_requests_list_idx
    ON native_requests (updated_at DESC, public_api_id);
CREATE INDEX native_requests_owner_idx
    ON native_requests (created_by_user_id, updated_at DESC, public_api_id);
CREATE INDEX native_requests_targets_idx
    ON native_requests (
        target_artist_public_api_id,
        target_release_public_api_id,
        target_track_public_api_id
    );
CREATE INDEX native_requests_auto_completion_idx
    ON native_requests (category, status)
    WHERE status IN (1, 2);

CREATE TRIGGER native_requests_public_api_id_immutable
BEFORE UPDATE OF public_api_id ON native_requests
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();

CREATE TABLE native_request_comments (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    request_id uuid NOT NULL REFERENCES native_requests (id) ON DELETE CASCADE,
    parent_comment_id uuid NULL,
    created_by_user_id uuid NULL REFERENCES native_users (id) ON DELETE SET NULL,
    body text NOT NULL,
    is_system boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,
    CONSTRAINT native_request_comments_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT native_request_comments_body_check CHECK (
        btrim(body) <> '' AND char_length(body) <= 10000
    ),
    UNIQUE (id, request_id),
    CONSTRAINT native_request_comments_parent_same_request_fk
        FOREIGN KEY (parent_comment_id, request_id)
        REFERENCES native_request_comments (id, request_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX native_request_comments_page_idx
    ON native_request_comments (request_id, created_at, public_api_id);

CREATE TRIGGER native_request_comments_public_api_id_immutable
BEFORE UPDATE OF public_api_id ON native_request_comments
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();

CREATE TABLE native_request_seen (
    request_id uuid NOT NULL REFERENCES native_requests (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    seen_at timestamptz NOT NULL,
    PRIMARY KEY (request_id, user_id)
);

CREATE TABLE native_shares (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    share_unique_id text NOT NULL UNIQUE,
    owner_user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    share_type smallint NOT NULL,
    resource_public_api_id uuid NOT NULL,
    description text NULL,
    is_downloadable boolean NOT NULL DEFAULT false,
    visit_count integer NOT NULL DEFAULT 0,
    expires_at timestamptz NULL,
    last_visited_at timestamptz NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_shares_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT native_shares_type_check CHECK (share_type BETWEEN 1 AND 4),
    CONSTRAINT native_shares_unique_id_check CHECK (
        share_unique_id ~ '^[0-9A-Fa-f]{32}$'
    ),
    CONSTRAINT native_shares_description_check CHECK (
        description IS NULL OR char_length(description) <= 10000
    ),
    CONSTRAINT native_shares_visit_count_check CHECK (visit_count >= 0),
    CONSTRAINT native_shares_time_check CHECK (
        created_at <= updated_at
        AND (last_visited_at IS NULL OR created_at <= last_visited_at)
    )
);

CREATE INDEX native_shares_owner_idx
    ON native_shares (owner_user_id, created_at DESC, public_api_id);

CREATE TRIGGER native_shares_public_api_id_immutable
BEFORE UPDATE OF public_api_id ON native_shares
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();

CREATE TABLE native_playback_settings (
    user_id uuid PRIMARY KEY REFERENCES native_users (id) ON DELETE CASCADE,
    crossfade_duration double precision NOT NULL DEFAULT 0,
    gapless_playback boolean NOT NULL DEFAULT true,
    volume_normalization boolean NOT NULL DEFAULT false,
    replay_gain text NOT NULL DEFAULT 'none',
    audio_quality text NOT NULL DEFAULT 'high',
    equalizer_preset text NULL,
    last_used_device text NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_playback_settings_crossfade_check CHECK (
        crossfade_duration BETWEEN 0 AND 30
    ),
    CONSTRAINT native_playback_settings_replay_gain_check CHECK (
        replay_gain IN ('none', 'track', 'album')
    ),
    CONSTRAINT native_playback_settings_quality_check CHECK (
        audio_quality IN ('low', 'medium', 'high', 'lossless')
    ),
    CONSTRAINT native_playback_settings_text_check CHECK (
        (equalizer_preset IS NULL OR char_length(equalizer_preset) <= 500)
        AND (last_used_device IS NULL OR char_length(last_used_device) <= 500)
    ),
    CONSTRAINT native_playback_settings_time_check CHECK (created_at <= updated_at)
);

CREATE TABLE native_equalizer_presets (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    name text NOT NULL,
    bands jsonb NOT NULL,
    is_default boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_equalizer_presets_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT native_equalizer_presets_name_check CHECK (
        btrim(name) <> '' AND char_length(name) <= 500
    ),
    CONSTRAINT native_equalizer_presets_bands_check CHECK (
        jsonb_typeof(bands) = 'array' AND jsonb_array_length(bands) BETWEEN 1 AND 64
    ),
    CONSTRAINT native_equalizer_presets_time_check CHECK (created_at <= updated_at),
    UNIQUE (user_id, name)
);

CREATE UNIQUE INDEX native_equalizer_presets_one_default_idx
    ON native_equalizer_presets (user_id) WHERE is_default;
CREATE INDEX native_equalizer_presets_list_idx
    ON native_equalizer_presets (user_id, created_at, public_api_id);

CREATE TRIGGER native_equalizer_presets_public_api_id_immutable
BEFORE UPDATE OF public_api_id ON native_equalizer_presets
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();
