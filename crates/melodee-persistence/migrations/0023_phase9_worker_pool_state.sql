-- worker-pool durable pause/resume control (ADR-0134 §8, PRD-WORK-005).
--
-- Additive. A binary downgrade leaves the table intact; schema rollback
-- restores a verified pre-Phase-9 backup into a new database.
--
-- Pause semantics: workers check their pool row between claim batches. A
-- paused pool stops claiming new work; in-flight claims finish or are
-- released per the existing lease contract, so pausing never strands a lease.
-- `paused_by`/`paused_at` are retained after resume as the durable last-pause
-- audit; `resume_requested_at` records the most recent resume request.

CREATE TABLE worker_pool_state (
    pool text PRIMARY KEY,
    paused boolean NOT NULL DEFAULT false,
    paused_by text NULL,
    paused_at timestamptz NULL,
    resume_requested_at timestamptz NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT worker_pool_state_pool_check CHECK (
        pool ~ '^[a-z][a-z0-9_-]{0,63}$'
    ),
 -- A paused pool always carries its operator audit, and the audit fields
 -- are always set (or cleared) together.
    CONSTRAINT worker_pool_state_pause_audit_check CHECK (
        (paused_by IS NULL) = (paused_at IS NULL)
        AND (paused_by IS NULL OR btrim(paused_by) <> '')
        AND (NOT paused OR paused_at IS NOT NULL)
    )
);

-- Seed exactly the pools whose workers honor the pause row in this phase
-- (podcast, policy, discovery). Pausing any other name lazily registers the
-- row (upsert) so newly wired pools need no migration; a pool without a row
-- is never paused.
INSERT INTO worker_pool_state (pool) VALUES
    ('discovery'),
    ('podcast'),
    ('policy');
