-- MEL-2 / ADR-0178: durable C#-equivalent account profile fields and audit.
-- This is additive. A binary downgrade leaves these columns and audit rows
-- intact; restore from a verified backup is the rollback procedure.

ALTER TABLE native_users
    ADD COLUMN description text,
    ADD COLUMN time_zone_id text NOT NULL DEFAULT 'UTC';

ALTER TABLE native_users
    ADD CONSTRAINT native_users_description_length_check
        CHECK (description IS NULL OR char_length(description) <= 62000),
    ADD CONSTRAINT native_users_time_zone_id_check
        CHECK (char_length(time_zone_id) BETWEEN 1 AND 64);

CREATE TABLE native_user_profile_audits (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE CASCADE,
    actor_user_id uuid NOT NULL REFERENCES native_users (id) ON DELETE RESTRICT,
    event text NOT NULL,
    changed_fields text[] NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT native_user_profile_audits_id_v7_check
        CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT native_user_profile_audits_event_check
        CHECK (btrim(event) <> '' AND char_length(event) <= 64),
    CONSTRAINT native_user_profile_audits_fields_check
        CHECK (cardinality(changed_fields) BETWEEN 1 AND 32),
    CONSTRAINT native_user_profile_audits_time_check
        CHECK (created_at IS NOT NULL)
);

CREATE INDEX native_user_profile_audits_user_created_idx
    ON native_user_profile_audits (user_id, created_at DESC, id DESC);
