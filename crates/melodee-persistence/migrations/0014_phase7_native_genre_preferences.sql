-- Phase 7 native-v1 per-user genre preferences (ADR-0127;
-- PRD-API-M1-004, PRD-USER-001..002).
--
-- Genre names are compatibility values, not Artist or catalog identities.
-- This additive table preserves the exact display value while using a
-- normalized value only to prevent duplicate preferences for case variants.

CREATE TABLE native_genre_user_states (
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    genre_name text NOT NULL,
    normalized_genre_name text NOT NULL,
    starred boolean NOT NULL DEFAULT false,
    hated boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (user_id, normalized_genre_name),
    CONSTRAINT native_genre_user_states_name_check CHECK (
        btrim(genre_name) <> ''
        AND char_length(genre_name) <= 500
        AND normalized_genre_name = lower(btrim(genre_name))
    ),
    CONSTRAINT native_genre_user_states_time_check CHECK (created_at <= updated_at)
);

CREATE INDEX native_genre_user_states_starred_idx
    ON native_genre_user_states (user_id, updated_at DESC, normalized_genre_name)
    WHERE starred;

CREATE INDEX native_genre_user_states_hated_idx
    ON native_genre_user_states (user_id, updated_at DESC, normalized_genre_name)
    WHERE hated;
