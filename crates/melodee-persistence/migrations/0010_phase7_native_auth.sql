-- native authentication, access control, and public-ID migration
-- authority (ADR-0124, ADR-0127).
--
-- This migration is additive. A binary downgrade leaves the tables intact;
-- schema rollback restores a verified pre-Phase-7 backup into a new database.
-- Clear authentication tokens are never stored.

CREATE TABLE native_users (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    username text NOT NULL,
    normalized_username text NOT NULL UNIQUE,
    email text NOT NULL,
    normalized_email text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    password_hash_algorithm text NOT NULL,
    stream_signing_key text NOT NULL,
    is_admin boolean NOT NULL DEFAULT false,
    is_editor boolean NOT NULL DEFAULT false,
    enabled boolean NOT NULL DEFAULT true,
    locked boolean NOT NULL DEFAULT false,
    blacklisted boolean NOT NULL DEFAULT false,
    preferred_language text NULL,
    preferred_theme text NULL,
    image_mime_type text NULL,
    image_bytes bytea NULL,
    auth_revision bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT native_users_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT native_users_username_check CHECK (
        btrim(username) <> '' AND btrim(normalized_username) <> ''
    ),
    CONSTRAINT native_users_email_check CHECK (
        btrim(email) <> '' AND btrim(normalized_email) <> ''
    ),
    CONSTRAINT native_users_password_check CHECK (
        btrim(password_hash) <> ''
        AND password_hash_algorithm = 'bcrypt'
    ),
    CONSTRAINT native_users_stream_key_check CHECK (btrim(stream_signing_key) <> ''),
    CONSTRAINT native_users_image_check CHECK (
        (image_mime_type IS NULL AND image_bytes IS NULL)
        OR (btrim(image_mime_type) <> '' AND image_bytes IS NOT NULL
            AND octet_length(image_bytes) BETWEEN 1 AND 10485760)
    ),
    CONSTRAINT native_users_auth_revision_check CHECK (auth_revision > 0),
    CONSTRAINT native_users_time_check CHECK (created_at <= updated_at)
);

CREATE FUNCTION reject_native_user_identity_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id OR NEW.public_api_id IS DISTINCT FROM OLD.public_api_id THEN
        RAISE EXCEPTION 'native user identity is immutable' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER native_users_identity_immutable
BEFORE UPDATE OF id, public_api_id ON native_users
FOR EACH ROW EXECUTE FUNCTION reject_native_user_identity_rewrite();

CREATE TABLE native_user_roles (
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    role text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (user_id, role),
    CONSTRAINT native_user_roles_value_check CHECK (btrim(role) <> '')
);

CREATE INDEX native_user_roles_role_idx ON native_user_roles (role, user_id);

CREATE TABLE native_library_access_groups (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    name text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT native_library_access_groups_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT native_library_access_groups_name_check CHECK (btrim(name) <> ''),
    CONSTRAINT native_library_access_groups_time_check CHECK (created_at <= updated_at)
);

CREATE TABLE native_user_library_groups (
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    group_id uuid NOT NULL REFERENCES native_library_access_groups (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (user_id, group_id)
);

CREATE TABLE native_library_group_libraries (
    group_id uuid NOT NULL REFERENCES native_library_access_groups (id) ON DELETE CASCADE,
    library_id uuid NOT NULL,
    imported_at timestamptz NOT NULL,
    PRIMARY KEY (group_id, library_id)
);
CREATE INDEX native_library_group_libraries_library_idx
    ON native_library_group_libraries (library_id, group_id);

-- NULL/unrestricted and libraries without an explicit binding preserve the
-- captured open-access behavior. Once any group binds a library, membership
-- in at least one bound group is required.
CREATE FUNCTION native_user_can_access_library(p_user_id uuid, p_library_id uuid)
RETURNS boolean LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT p_library_id IS NULL
        OR NOT EXISTS (
            SELECT 1 FROM native_library_group_libraries binding
            WHERE binding.library_id = p_library_id
        )
        OR EXISTS (
            SELECT 1 FROM native_library_group_libraries binding
            JOIN native_user_library_groups membership
              ON membership.group_id = binding.group_id
            WHERE binding.library_id = p_library_id
              AND membership.user_id = p_user_id
        )
$$;

CREATE FUNCTION native_user_can_access_release(p_user_id uuid, p_release_id uuid)
RETURNS boolean LANGUAGE sql STABLE PARALLEL SAFE AS $$
    SELECT COALESCE((
        SELECT native_user_can_access_library(p_user_id, work_item.source_library_id)
        FROM catalog_imports catalog_import
        JOIN release_work_items work_item ON work_item.id = catalog_import.work_item_id
        WHERE catalog_import.release_id = p_release_id
    ), true)
$$;

CREATE FUNCTION native_user_can_access_artist(p_user_id uuid, p_artist_id uuid)
RETURNS boolean LANGUAGE sql STABLE PARALLEL SAFE SECURITY INVOKER AS $$
    SELECT EXISTS (
        SELECT 1 FROM release_artists release_artist
        WHERE release_artist.artist_id = p_artist_id
          AND native_user_can_access_release(p_user_id, release_artist.release_id)
        UNION ALL
        SELECT 1 FROM track_artists track_artist
        JOIN tracks track ON track.id = track_artist.track_id
        WHERE track_artist.artist_id = p_artist_id
          AND native_user_can_access_release(p_user_id, track.release_id)
    )
$$;

CREATE TABLE native_refresh_families (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    device_id text NULL,
    session_started_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz NULL,
    revocation_reason text NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT native_refresh_families_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT native_refresh_families_device_check CHECK (
        device_id IS NULL OR (btrim(device_id) <> '' AND char_length(device_id) <= 256)
    ),
    CONSTRAINT native_refresh_families_expiry_check CHECK (
        session_started_at < expires_at
        AND created_at = session_started_at
        AND created_at <= updated_at
        AND (revoked_at IS NULL OR revoked_at >= session_started_at)
    ),
    CONSTRAINT native_refresh_families_revocation_check CHECK (
        (revoked_at IS NULL AND revocation_reason IS NULL)
        OR (revoked_at IS NOT NULL AND btrim(revocation_reason) <> '')
    )
);

CREATE INDEX native_refresh_families_user_active_idx
    ON native_refresh_families (user_id, expires_at, id)
    WHERE revoked_at IS NULL;

CREATE TABLE native_refresh_tokens (
    id uuid PRIMARY KEY,
    family_id uuid NOT NULL REFERENCES native_refresh_families (id) ON DELETE CASCADE,
    token_digest bytea NOT NULL UNIQUE,
    issued_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz NULL,
    revoked_at timestamptz NULL,
    revocation_reason text NULL,
    replacement_token_id uuid NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT native_refresh_tokens_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT native_refresh_tokens_digest_check CHECK (octet_length(token_digest) = 32),
    CONSTRAINT native_refresh_tokens_expiry_check CHECK (
        issued_at < expires_at AND created_at = issued_at
    ),
    CONSTRAINT native_refresh_tokens_consumption_check CHECK (
        (consumed_at IS NULL AND replacement_token_id IS NULL)
        OR (
            consumed_at IS NOT NULL
            AND consumed_at >= issued_at
            AND replacement_token_id IS NOT NULL
        )
    ),
    CONSTRAINT native_refresh_tokens_revocation_check CHECK (
        (revoked_at IS NULL AND revocation_reason IS NULL)
        OR (
            revoked_at IS NOT NULL
            AND revoked_at >= issued_at
            AND btrim(revocation_reason) <> ''
        )
    ),
    CONSTRAINT native_refresh_tokens_replacement_check CHECK (replacement_token_id IS DISTINCT FROM id)
);

ALTER TABLE native_refresh_tokens
    ADD CONSTRAINT native_refresh_tokens_replacement_fk
    FOREIGN KEY (replacement_token_id) REFERENCES native_refresh_tokens (id)
    ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

CREATE UNIQUE INDEX native_refresh_tokens_replacement_unique_idx
    ON native_refresh_tokens (replacement_token_id)
    WHERE replacement_token_id IS NOT NULL;
CREATE INDEX native_refresh_tokens_family_idx
    ON native_refresh_tokens (family_id, issued_at, id);

CREATE TABLE native_password_reset_tokens (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    token_digest bytea NOT NULL UNIQUE,
    issued_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz NULL,
    invalidated_at timestamptz NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT native_password_reset_id_v7 CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT native_password_reset_digest CHECK (octet_length(token_digest) = 32),
    CONSTRAINT native_password_reset_time CHECK (
        issued_at < expires_at AND created_at = issued_at
        AND (consumed_at IS NULL OR consumed_at >= issued_at)
        AND (invalidated_at IS NULL OR invalidated_at >= issued_at)
        AND NOT (consumed_at IS NOT NULL AND invalidated_at IS NOT NULL)
    )
);
CREATE INDEX native_password_reset_user_active_idx
    ON native_password_reset_tokens (user_id, expires_at, id)
    WHERE consumed_at IS NULL AND invalidated_at IS NULL;

CREATE TABLE native_user_social_logins (
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    provider text NOT NULL,
    provider_subject text NOT NULL,
    verified_email text NULL,
    display_name text NULL,
    hosted_domain text NULL,
    linked_at timestamptz NOT NULL,
    last_login_at timestamptz NOT NULL,
    PRIMARY KEY (provider, provider_subject),
    UNIQUE (user_id, provider),
    CONSTRAINT native_social_provider CHECK (
        provider = lower(provider) AND provider ~ '^[a-z][a-z0-9_-]{0,49}$'
    ),
    CONSTRAINT native_social_subject CHECK (
        btrim(provider_subject) <> '' AND char_length(provider_subject) <= 500
    ),
    CONSTRAINT native_social_time CHECK (linked_at <= last_login_at)
);

CREATE FUNCTION validate_native_refresh_token_family() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    family_started_at timestamptz;
    family_expires_at timestamptz;
    replacement_family_id uuid;
BEGIN
    SELECT session_started_at, expires_at
      INTO family_started_at, family_expires_at
      FROM native_refresh_families WHERE id = NEW.family_id;
    IF NOT FOUND OR NEW.issued_at < family_started_at OR NEW.expires_at > family_expires_at THEN
        RAISE EXCEPTION 'refresh token lies outside its family lifetime' USING ERRCODE = '23514';
    END IF;
    IF NEW.replacement_token_id IS NOT NULL THEN
        SELECT family_id INTO replacement_family_id
          FROM native_refresh_tokens WHERE id = NEW.replacement_token_id;
        IF NOT FOUND OR replacement_family_id <> NEW.family_id THEN
            RAISE EXCEPTION 'refresh replacement must remain in its token family'
                USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER native_refresh_tokens_family_lifetime
AFTER INSERT OR UPDATE ON native_refresh_tokens
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION validate_native_refresh_token_family();

CREATE FUNCTION validate_native_refresh_family_tokens() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM native_refresh_tokens token
        WHERE token.family_id = NEW.id
          AND (
              token.issued_at < NEW.session_started_at
              OR token.expires_at > NEW.expires_at
          )
    ) THEN
        RAISE EXCEPTION 'refresh family lifetime excludes an existing token'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER native_refresh_family_token_lifetime
AFTER UPDATE OF session_started_at, expires_at ON native_refresh_families
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION validate_native_refresh_family_tokens();

CREATE TABLE native_migration_identity_bindings (
    source_kind text NOT NULL,
    source_key text NOT NULL,
    internal_id uuid NOT NULL,
    public_api_id uuid NOT NULL,
    imported_at timestamptz NOT NULL,
    PRIMARY KEY (source_kind, source_key),
    UNIQUE (source_kind, internal_id),
    UNIQUE (source_kind, public_api_id),
    CONSTRAINT native_migration_identity_kind_check CHECK (
        source_kind IN ('user', 'artist', 'release', 'track', 'playlist', 'share', 'session')
    ),
    CONSTRAINT native_migration_identity_source_check CHECK (btrim(source_key) <> '')
);

-- Serializes the bounded-key admission decision across server processes.
CREATE TABLE native_rate_limit_state (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    created_at timestamptz NOT NULL
);
INSERT INTO native_rate_limit_state (singleton, created_at) VALUES (true, now());

CREATE TABLE native_rate_limit_counters (
    scope text NOT NULL,
    actor_key_digest bytea NOT NULL,
    attempt_count integer NOT NULL,
    window_started_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (scope, actor_key_digest),
    CONSTRAINT native_rate_limit_scope_check CHECK (
        btrim(scope) <> '' AND char_length(scope) <= 100
    ),
    CONSTRAINT native_rate_limit_digest_check CHECK (octet_length(actor_key_digest) = 32),
    CONSTRAINT native_rate_limit_attempt_check CHECK (attempt_count > 0),
    CONSTRAINT native_rate_limit_time_check CHECK (window_started_at < expires_at)
);
CREATE INDEX native_rate_limit_counters_expiry_idx
    ON native_rate_limit_counters (expires_at, scope, actor_key_digest);
