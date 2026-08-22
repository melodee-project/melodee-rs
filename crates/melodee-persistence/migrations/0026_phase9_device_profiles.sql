-- device/transcoding profile metadata (ADR-0134 §5, PRD-DEVICE-001).
--
-- Additive. A binary downgrade leaves the table intact; schema rollback
-- restores a verified pre-migration backup into a new database.
--
-- Rows are operator-managed METADATA ONLY (PRD-DEVICE-002): no column here
-- feeds stream negotiation, and the admin API documenting them must not claim
-- transcoding control. Profile-driven stream selection is post-parity work
-- requiring its own ADR (PRD-DEVICE-003).
--
-- Legacy parity (evidence:
-- design/evidence/phase-9/device-profiles-legacy-inventory.md): the legacy
-- preview model bounded names at 255 characters and allowed exactly one
-- default; both invariants are enforced here as constraints rather than
-- service-level convention.

CREATE TABLE device_profiles (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    -- Free-form client matcher metadata (e.g. {"clientName": "...",
    -- "userAgentContains": "..."}). Must be a JSON object; shape and value
    -- bounds are validated in code before insert, these CHECKs are the
    -- fail-closed backstop.
    client_match jsonb NOT NULL DEFAULT '{}'::jsonb,
    -- Transcoding-intent metadata (e.g. {"directPlay": true}). Must be a JSON
    -- object; legacy consistency rules (direct play vs codec/bitrate) are
    -- validated in code. Never consumed by streaming in this phase.
    profile jsonb NOT NULL DEFAULT '{}'::jsonb,
    is_default boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT device_profiles_name_check CHECK (
        btrim(name) <> '' AND char_length(name) <= 255
    ),
    CONSTRAINT device_profiles_client_match_object_check CHECK (
        jsonb_typeof(client_match) = 'object'
    ),
    CONSTRAINT device_profiles_profile_object_check CHECK (
        jsonb_typeof(profile) = 'object'
    ),
    CONSTRAINT device_profiles_client_match_size_check CHECK (
        octet_length(client_match::text) <= 4096
    ),
    CONSTRAINT device_profiles_profile_size_check CHECK (
        octet_length(profile::text) <= 4096
    )
);

-- At most one default profile server-wide (legacy parity: setting a default
-- clears the previous one; here the invariant is fail-closed at the database).
CREATE UNIQUE INDEX device_profiles_single_default
    ON device_profiles (is_default) WHERE is_default;

-- Deterministic operator listing order: default first, then name.
CREATE INDEX device_profiles_listing ON device_profiles (is_default DESC, name);
