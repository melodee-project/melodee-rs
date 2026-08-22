-- native-v1 podcast subscriptions and playback state
-- (ADR-0123; PRD-POD-001..003).
--
-- Additive only. Binary downgrade retains these tables. Full schema rollback
-- restores a verified pre-0013 backup into a new database. Enclosure downloads
-- are explicit queued state; this migration never reads or deletes media.

CREATE TABLE native_podcast_channels (
    id bigserial PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    owner_user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    feed_url text NOT NULL,
    title text NOT NULL,
    description text NOT NULL DEFAULT '',
    image_url text NOT NULL DEFAULT '',
    website text NOT NULL DEFAULT '',
    tags text NOT NULL DEFAULT '',
    auto_download_enabled boolean NOT NULL DEFAULT false,
    refresh_interval_hours integer NULL,
    max_downloaded_episodes integer NULL,
    max_storage_bytes bigint NULL,
    last_sync_at timestamptz NULL,
    soft_deleted_at timestamptz NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_podcast_channels_feed_check CHECK (
        btrim(feed_url) <> '' AND char_length(feed_url) <= 4096
    ),
    CONSTRAINT native_podcast_channels_title_check CHECK (
        btrim(title) <> '' AND char_length(title) <= 1000
    ),
    CONSTRAINT native_podcast_channels_description_check CHECK (char_length(description) <= 100000),
    CONSTRAINT native_podcast_channels_refresh_check CHECK (
        refresh_interval_hours IS NULL OR refresh_interval_hours BETWEEN 1 AND 8760
    ),
    CONSTRAINT native_podcast_channels_download_count_check CHECK (
        max_downloaded_episodes IS NULL OR max_downloaded_episodes BETWEEN 0 AND 100000
    ),
    CONSTRAINT native_podcast_channels_storage_check CHECK (
        max_storage_bytes IS NULL OR max_storage_bytes >= 0
    ),
    CONSTRAINT native_podcast_channels_time_check CHECK (created_at <= updated_at),
    UNIQUE (owner_user_id, feed_url)
);

CREATE INDEX native_podcast_channels_list_idx
    ON native_podcast_channels (owner_user_id, soft_deleted_at, title, id);

CREATE UNIQUE INDEX native_podcast_channels_owner_feed_normalized_idx
    ON native_podcast_channels (owner_user_id, lower(feed_url));

CREATE TRIGGER native_podcast_channels_public_api_id_immutable
BEFORE UPDATE OF public_api_id ON native_podcast_channels
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();

CREATE TABLE native_podcast_episodes (
    id bigserial PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    channel_id bigint NOT NULL REFERENCES native_podcast_channels (id) ON DELETE CASCADE,
    title text NOT NULL,
    description text NOT NULL DEFAULT '',
    published_at timestamptz NULL,
    feed_item_id text NOT NULL,
    duration_ms bigint NULL,
    enclosure_url text NULL,
    local_path text NULL,
    download_status text NOT NULL DEFAULT 'NotDownloaded',
    download_error text NULL,
    download_attempt_count integer NOT NULL DEFAULT 0,
    download_available_at timestamptz NOT NULL DEFAULT now(),
    download_lease_owner uuid NULL,
    download_lease_expires_at timestamptz NULL,
    downloaded_bytes bigint NULL,
    downloaded_at timestamptz NULL,
    soft_deleted_at timestamptz NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_podcast_episodes_title_check CHECK (
        btrim(title) <> '' AND char_length(title) <= 2000
    ),
    CONSTRAINT native_podcast_episodes_description_check CHECK (char_length(description) <= 200000),
    CONSTRAINT native_podcast_episodes_duration_check CHECK (duration_ms IS NULL OR duration_ms >= 0),
    CONSTRAINT native_podcast_episodes_feed_item_check CHECK (
        btrim(feed_item_id) <> '' AND char_length(feed_item_id) <= 4096
    ),
    CONSTRAINT native_podcast_episodes_enclosure_check CHECK (
        enclosure_url IS NULL OR char_length(enclosure_url) <= 4096
    ),
    CONSTRAINT native_podcast_episodes_download_status_check CHECK (
        download_status IN ('NotDownloaded', 'Queued', 'Downloading', 'Downloaded', 'Failed')
    ),
    CONSTRAINT native_podcast_episodes_download_attempt_check CHECK (
        download_attempt_count BETWEEN 0 AND 100
    ),
    CONSTRAINT native_podcast_episodes_download_lease_check CHECK (
        (download_status = 'Downloading' AND download_lease_owner IS NOT NULL AND download_lease_expires_at IS NOT NULL)
        OR (download_status <> 'Downloading' AND download_lease_owner IS NULL AND download_lease_expires_at IS NULL)
    ),
    CONSTRAINT native_podcast_episodes_downloaded_check CHECK (
        downloaded_bytes IS NULL OR downloaded_bytes >= 0
    ),
    CONSTRAINT native_podcast_episodes_time_check CHECK (created_at <= updated_at),
    UNIQUE (channel_id, feed_item_id)
);

CREATE INDEX native_podcast_episodes_list_idx
    ON native_podcast_episodes (channel_id, soft_deleted_at, published_at DESC, id);
CREATE INDEX native_podcast_episodes_search_idx
    ON native_podcast_episodes USING gin (
        to_tsvector('simple', coalesce(title, '') || ' ' || coalesce(description, ''))
    );
CREATE INDEX native_podcast_episodes_download_claim_idx
    ON native_podcast_episodes (download_available_at, id)
    WHERE download_status IN ('Queued', 'Downloading');

CREATE TRIGGER native_podcast_episodes_public_api_id_immutable
BEFORE UPDATE OF public_api_id ON native_podcast_episodes
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();

CREATE TABLE native_podcast_bookmarks (
    id bigserial PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    episode_id bigint NOT NULL REFERENCES native_podcast_episodes (id) ON DELETE CASCADE,
    position_seconds integer NOT NULL,
    comment text NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_podcast_bookmarks_position_check CHECK (position_seconds >= 0),
    CONSTRAINT native_podcast_bookmarks_comment_check CHECK (
        comment IS NULL OR char_length(comment) <= 10000
    ),
    CONSTRAINT native_podcast_bookmarks_time_check CHECK (created_at <= updated_at),
    UNIQUE (user_id, episode_id)
);

CREATE TABLE native_podcast_play_history (
    id bigserial PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    episode_id bigint NOT NULL REFERENCES native_podcast_episodes (id) ON DELETE CASCADE,
    played_at timestamptz NOT NULL,
    client text NOT NULL,
    by_user_agent text NULL,
    ip_address text NULL,
    seconds_played integer NULL,
    source smallint NOT NULL DEFAULT 0,
    is_now_playing boolean NOT NULL DEFAULT false,
    last_heartbeat_at timestamptz NULL,
    CONSTRAINT native_podcast_play_history_client_check CHECK (
        btrim(client) <> '' AND char_length(client) <= 500
    ),
    CONSTRAINT native_podcast_play_history_seconds_check CHECK (
        seconds_played IS NULL OR seconds_played >= 0
    )
);

CREATE INDEX native_podcast_play_history_list_idx
    ON native_podcast_play_history (user_id, episode_id, played_at DESC, id DESC);

CREATE UNIQUE INDEX native_podcast_one_now_playing_per_user_idx
    ON native_podcast_play_history (user_id) WHERE is_now_playing;
