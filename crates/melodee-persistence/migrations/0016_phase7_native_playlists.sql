-- native playlist metadata and smart playlists (ADR-0114, ADR-0127).

ALTER TABLE native_playlists
    ADD COLUMN image_mime_type text NULL,
    ADD COLUMN image_bytes bytea NULL,
    ADD CONSTRAINT native_playlists_image_check CHECK (
        (image_mime_type IS NULL AND image_bytes IS NULL)
        OR (image_mime_type IN ('image/jpeg', 'image/png', 'image/webp')
            AND image_bytes IS NOT NULL
            AND octet_length(image_bytes) BETWEEN 1 AND 10485760)
    );

CREATE TABLE native_smart_playlists (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    owner_user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    name text NOT NULL,
    mql_query text NOT NULL,
    entity_type text NOT NULL,
    last_result_count integer NOT NULL DEFAULT 0,
    last_evaluated_at timestamptz NULL,
    is_public boolean NOT NULL DEFAULT false,
    normalized_query text NULL,
    revision bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_smart_playlists_id_v7 CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT native_smart_playlists_name CHECK (
        btrim(name) <> '' AND char_length(name) <= 255
    ),
    CONSTRAINT native_smart_playlists_query CHECK (
        btrim(mql_query) <> '' AND char_length(mql_query) <= 4000
        AND (normalized_query IS NULL OR char_length(normalized_query) <= 4000)
    ),
    CONSTRAINT native_smart_playlists_entity CHECK (
        entity_type IN ('songs', 'albums', 'artists')
    ),
    CONSTRAINT native_smart_playlists_count CHECK (last_result_count >= 0),
    CONSTRAINT native_smart_playlists_revision CHECK (revision > 0),
    CONSTRAINT native_smart_playlists_time CHECK (created_at <= updated_at),
    UNIQUE (owner_user_id, name)
);

CREATE INDEX native_smart_playlists_owner_idx
    ON native_smart_playlists (owner_user_id, created_at DESC, public_api_id);
CREATE INDEX native_smart_playlists_public_idx
    ON native_smart_playlists (created_at DESC, public_api_id) WHERE is_public;

CREATE TRIGGER native_smart_playlists_public_api_id_immutable
BEFORE UPDATE OF public_api_id ON native_smart_playlists
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();
