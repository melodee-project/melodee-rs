-- Phase 8 Jellyfin adapter token state (authority: ADR-0102, ADR-0131).
--
-- This migration is additive. A binary downgrade leaves the table intact;
-- schema rollback restores a verified pre-Phase-8 backup into a new database.
-- Clear access tokens are never stored: lookup uses the 8-character cleartext
-- prefix and verification uses HMAC-SHA256(pepper, salt + token), matching the
-- captured legacy storage contract.

CREATE TABLE jellyfin_access_tokens (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    token_prefix text NOT NULL,
    token_digest bytea NOT NULL UNIQUE,
    token_salt bytea NOT NULL,
    client text NULL,
    device_name text NULL,
    device_id text NULL,
    issued_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT jellyfin_access_tokens_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT jellyfin_access_tokens_prefix_check CHECK (
        char_length(token_prefix) = 8 AND token_prefix ~ '^[0-9a-f]{8}$'
    ),
    CONSTRAINT jellyfin_access_tokens_digest_check CHECK (octet_length(token_digest) = 32),
    CONSTRAINT jellyfin_access_tokens_salt_check CHECK (octet_length(token_salt) = 16),
    CONSTRAINT jellyfin_access_tokens_client_check CHECK (
        (client IS NULL OR char_length(client) <= 200)
        AND (device_name IS NULL OR char_length(device_name) <= 200)
        AND (device_id IS NULL OR char_length(device_id) <= 200)
    ),
    CONSTRAINT jellyfin_access_tokens_expiry_check CHECK (
        issued_at < expires_at
        AND created_at = issued_at
        AND created_at <= updated_at
        AND (revoked_at IS NULL OR revoked_at >= issued_at)
    )
);

CREATE INDEX jellyfin_access_tokens_prefix_idx
    ON jellyfin_access_tokens (token_prefix, id);

CREATE INDEX jellyfin_access_tokens_user_active_idx
    ON jellyfin_access_tokens (user_id, issued_at, id)
    WHERE revoked_at IS NULL;
