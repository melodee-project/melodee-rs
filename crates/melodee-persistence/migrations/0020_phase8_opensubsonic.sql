-- OpenSubsonic adapter state (authority: ADR-0102, ADR-0130).
--
-- This migration is additive. A binary downgrade leaves the tables intact;
-- schema rollback restores a verified pre-Phase-8 backup into a new database.
-- The per-user OpenSubsonic secret is an API token that must remain
-- recoverable so the server can recompute md5(secret + salt) for arbitrary
-- client-chosen salts; it is isolated from password hashes, never logged,
-- and redacted from captures (ADR-0130 decision 6).

CREATE TABLE opensubsonic_user_secrets (
    user_id uuid PRIMARY KEY REFERENCES native_users (id) ON DELETE CASCADE,
    secret text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT opensubsonic_user_secrets_secret_check CHECK (
        btrim(secret) <> '' AND char_length(secret) <= 256
    ),
    CONSTRAINT opensubsonic_user_secrets_time_check CHECK (created_at <= updated_at)
);

-- Bookmarks: captured CRUD over songs; the raw position value is preserved
-- verbatim because the legacy unit is inconsistent (controller doc ms, model
-- doc seconds).
CREATE TABLE opensubsonic_bookmarks (
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    position bigint NOT NULL,
    comment text NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (user_id, track_id),
    CONSTRAINT opensubsonic_bookmarks_position_check CHECK (position >= 0),
    CONSTRAINT opensubsonic_bookmarks_comment_check CHECK (
        comment IS NULL OR char_length(comment) <= 4000
    ),
    CONSTRAINT opensubsonic_bookmarks_time_check CHECK (created_at <= updated_at)
);

-- Play queue: one queue per user. The captured `current` value is the
-- legacy internal PlayQueId (an integer entry id, not a song id), so items
-- carry a stable bigint entry id.
CREATE TABLE opensubsonic_play_queue_items (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    queue_position integer NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT opensubsonic_play_queue_items_position_check CHECK (queue_position >= 0),
    CONSTRAINT opensubsonic_play_queue_items_order_unique UNIQUE (user_id, queue_position)
);

CREATE TABLE opensubsonic_play_queues (
    user_id uuid PRIMARY KEY REFERENCES native_users (id) ON DELETE CASCADE,
    current_entry_id bigint NULL REFERENCES opensubsonic_play_queue_items (id) ON DELETE SET NULL,
    position bigint NULL,
    changed_by text NOT NULL,
    changed_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT opensubsonic_play_queues_position_check CHECK (
        position IS NULL OR position >= 0
    ),
    CONSTRAINT opensubsonic_play_queues_changed_by_check CHECK (btrim(changed_by) <> ''),
    CONSTRAINT opensubsonic_play_queues_time_check CHECK (
        created_at <= updated_at AND changed_at >= created_at
    )
);

-- Internet radio stations: legacy ids are `radio_{guid}`; the public guid is
-- immutable once published.
CREATE TABLE opensubsonic_radio_stations (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    name text NOT NULL,
    stream_url text NOT NULL,
    home_page_url text NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT opensubsonic_radio_stations_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT opensubsonic_radio_stations_name_check CHECK (
        btrim(name) <> '' AND char_length(name) <= 500
    ),
    CONSTRAINT opensubsonic_radio_stations_stream_url_check CHECK (
        btrim(stream_url) <> '' AND char_length(stream_url) <= 2000
    ),
    CONSTRAINT opensubsonic_radio_stations_home_page_check CHECK (
        home_page_url IS NULL OR char_length(home_page_url) <= 2000
    ),
    CONSTRAINT opensubsonic_radio_stations_time_check CHECK (created_at <= updated_at)
);

CREATE FUNCTION reject_opensubsonic_radio_station_identity_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id OR NEW.public_api_id IS DISTINCT FROM OLD.public_api_id THEN
        RAISE EXCEPTION 'opensubsonic radio station identity is immutable' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER opensubsonic_radio_stations_identity_immutable
BEFORE UPDATE OF id, public_api_id ON opensubsonic_radio_stations
FOR EACH ROW EXECUTE FUNCTION reject_opensubsonic_radio_station_identity_rewrite();

-- Scan runs back startScan/getScanStatus. The captured baseline answers
-- scanning:true/count:0 when the scheduler is disabled; real runs record the
-- scanned track count.
CREATE TABLE opensubsonic_scan_runs (
    id uuid PRIMARY KEY,
    user_id uuid NULL REFERENCES native_users (id) ON DELETE SET NULL,
    started_at timestamptz NOT NULL,
    finished_at timestamptz NULL,
    track_count bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL,

    CONSTRAINT opensubsonic_scan_runs_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT opensubsonic_scan_runs_count_check CHECK (track_count >= 0),
    CONSTRAINT opensubsonic_scan_runs_time_check CHECK (
        created_at = started_at AND (finished_at IS NULL OR finished_at >= started_at)
    )
);

CREATE INDEX opensubsonic_scan_runs_latest_idx ON opensubsonic_scan_runs (started_at DESC, id);
