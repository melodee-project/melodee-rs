-- onboarding state and DB-backed server settings (ADR-0134 §3,
-- PRD-SETUP-001/002, PRD-CONFIG-001..003, PRD-AUTH-007).
--
-- Additive. A binary downgrade leaves both tables intact; schema rollback
-- restores a verified pre-Phase-9 backup into a new database.
--
-- Legacy parity (evidence: design/evidence/phase-9/configuration-inventory.md):
-- the legacy `Settings`
-- table stored every value as a string; the Rust store uses typed jsonb so
-- booleans/numbers validate structurally instead of by parse-at-read
-- convention. The 190-key classification (kinds, secret flags, seeded
-- defaults) lives in code (`crates/melodee-persistence/src/settings.rs`),
-- not as seed rows: env/file configuration always wins over this table
-- (PRD-CONFIG-003), so a built-in default is the bottom layer, not a row
-- that can drift from it.
--
-- Secret values (`is_secret`) are write-only over every API surface: reads,
-- listings, and exports return a fixed redaction marker; import never
-- accepts the marker as a value (PRD-AUTH-007). The plaintext at rest rule
-- for the migrated-installation path is unchanged from legacy (the legacy
-- table stored plaintext values too); encryption at rest is a deployment
-- concern, not a column.

CREATE TABLE server_settings (
    -- Dotted legacy-parity key (e.g. email.smtpHost). Keys are case
    -- sensitive and match the legacy registry strings verbatim, including
    -- the captured casing quirks (configuration-inventory.md "Parity
    -- quirks").
    key text PRIMARY KEY,
    value jsonb NOT NULL,
    is_secret boolean NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT server_settings_key_check CHECK (
        btrim(key) <> '' AND char_length(key) <= 255
    ),
    CONSTRAINT server_settings_value_size_check CHECK (
        octet_length(value::text) <= 65536
    )
);

-- Singleton onboarding marker (ADR-0134 §3): at most one row, `id` always
-- true. The row may exist with completed_at NULL while setup evidence
-- (configuration imports) accumulates before completion; a non-NULL
-- completed_at is the durable completion marker implementing the
-- PRD-SETUP-002 bypass together with the catalog-evidence heuristic in
-- code.
CREATE TABLE onboarding_state (
    id boolean PRIMARY KEY,
    completed_at timestamptz,
    -- Stable operator identity (administrator public id or CLI actor).
    completed_by text,
    -- Setup evidence: preflight check results and bounded configuration
    -- import audit entries. Never contains secret values (secret values are
    -- rejected by code before they could land here).
    evidence jsonb,

    CONSTRAINT onboarding_state_singleton_check CHECK (id),
    CONSTRAINT onboarding_state_completed_by_check CHECK (
        completed_by IS NULL OR (
            btrim(completed_by) <> '' AND char_length(completed_by) <= 255
        )
    ),
    CONSTRAINT onboarding_state_evidence_check CHECK (
        evidence IS NULL OR (
            jsonb_typeof(evidence) = 'object'
            AND octet_length(evidence::text) <= 65536
        )
    )
);
