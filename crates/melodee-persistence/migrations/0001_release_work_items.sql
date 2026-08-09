-- Phase 1A: durable Release work items (SPEC.md section 11.1).
--
-- Processing state only: this table is never the catalog Release entity.
-- All timestamps are UTC (timestamptz).

CREATE TABLE release_work_items (
    -- Internal identity (uuidv7: time-ordered, app-generated for determinism).
    id                   uuid PRIMARY KEY,
    -- Operator-safe public identity (stable, safe to display in UI/CLI/logs).
    api_key              uuid NOT NULL UNIQUE,

    source_library_id    uuid NULL,
    target_library_id    uuid NULL,

    source_path          text NOT NULL,
    current_path         text NOT NULL,
    target_path          text NULL,

    source_fingerprint   text NULL,
    manifest_hash        text NULL,

    -- CHECK keeps persisted states inside the domain enum (readable text form).
    state                text NOT NULL,
    state_reason         text NULL,

    attempt_count        integer NOT NULL DEFAULT 0,
    next_attempt_at      timestamptz NULL,

    lease_owner          text NULL,
    lease_acquired_at    timestamptz NULL,
    lease_expires_at     timestamptz NULL,

    last_error_code      text NULL,
    last_error_message   text NULL,

    correlation_id       uuid NOT NULL,

    discovered_at        timestamptz NOT NULL,
    first_started_at     timestamptz NULL,
    last_attempt_at      timestamptz NULL,
    updated_at           timestamptz NOT NULL,
    created_at           timestamptz NOT NULL,
    completed_at         timestamptz NULL,

    CONSTRAINT release_work_items_state_check CHECK (
        state IN (
            'discovered', 'settling', 'ready', 'inspecting', 'normalizing',
            'validating', 'needs_review', 'staged', 'publishing', 'published',
            'cataloging', 'cataloged', 'indexing', 'indexed', 'failed',
            'quarantined', 'ignored', 'deleted'
        )
    ),
    CONSTRAINT release_work_items_attempt_count_check CHECK (attempt_count >= 0),
    -- Lease fields are internally consistent: owner set iff both timestamps set.
    CONSTRAINT release_work_items_lease_consistency_check CHECK (
        (lease_owner IS NULL AND lease_acquired_at IS NULL AND lease_expires_at IS NULL)
        OR (lease_owner IS NOT NULL AND lease_acquired_at IS NOT NULL AND lease_expires_at IS NOT NULL)
    ),
    -- Retry timestamps only make sense for failed work awaiting another attempt.
    CONSTRAINT release_work_items_retry_consistency_check CHECK (
        next_attempt_at IS NULL OR state = 'failed'
    ),
    -- Terminal states have a completion timestamp; non-terminal states do not.
    CONSTRAINT release_work_items_completion_consistency_check CHECK (
        (state IN ('indexed', 'ignored', 'deleted') AND completed_at IS NOT NULL)
        OR (state NOT IN ('indexed', 'ignored', 'deleted') AND completed_at IS NULL)
    )
);

-- Claim query: state filter + retry eligibility + deterministic order.
-- Partial: only rows that can be claimed at all (ready, or failed awaiting retry).
CREATE INDEX release_work_items_claim_idx
    ON release_work_items (next_attempt_at NULLS FIRST, created_at, id)
    WHERE state IN ('ready', 'failed');

-- Lease recovery scan: find expired leases without scanning the whole table.
CREATE INDEX release_work_items_lease_expiry_idx
    ON release_work_items (lease_expires_at)
    WHERE lease_expires_at IS NOT NULL;

-- Operator/diagnostic views by state.
CREATE INDEX release_work_items_state_idx ON release_work_items (state);
