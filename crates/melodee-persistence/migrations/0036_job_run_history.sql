-- ADR-0158: durable tick-level scheduled-job history.
--
-- This migration is additive. Rollback is restore/forward migration: if a
-- deployment must be reverted, restore the pre-migration backup and rerun the
-- older binary. No catalog or source-media rows are touched by this change.

CREATE TABLE job_run_history (
    id uuid PRIMARY KEY,
    job_name text NOT NULL,
    scheduled_tick_at timestamptz NULL,
    started_at timestamptz NOT NULL,
    completed_at timestamptz NULL,
    duration_ms double precision NULL,
    success boolean NULL,
    error_message text NULL,
    was_manual_trigger boolean NOT NULL DEFAULT false,
    triggered_by text NULL,
    CONSTRAINT job_run_history_name_check CHECK (
        job_name ~ '^[a-z][a-z0-9_-]{0,63}$'
    ),
    CONSTRAINT job_run_history_duration_check CHECK (
        duration_ms IS NULL OR duration_ms >= 0
    ),
    CONSTRAINT job_run_history_completion_check CHECK (
        (completed_at IS NULL AND duration_ms IS NULL AND success IS NULL)
        OR (completed_at IS NOT NULL AND duration_ms IS NOT NULL AND success IS NOT NULL)
    ),
    CONSTRAINT job_run_history_failure_check CHECK (
        success IS NOT FALSE OR NULLIF(btrim(error_message), '') IS NOT NULL
    ),
    CONSTRAINT job_run_history_trigger_check CHECK (
        triggered_by IS NULL OR char_length(triggered_by) BETWEEN 1 AND 200
    )
);

CREATE UNIQUE INDEX job_run_history_tick_unique
    ON job_run_history (job_name, scheduled_tick_at)
    WHERE scheduled_tick_at IS NOT NULL AND was_manual_trigger = false;

CREATE INDEX job_run_history_job_started_idx
    ON job_run_history (job_name, started_at DESC, id DESC);

CREATE INDEX job_run_history_started_idx
    ON job_run_history (started_at DESC, id DESC);

CREATE TABLE job_run_history_cleanup_audit (
    id uuid PRIMARY KEY,
    retention_days integer NOT NULL,
    removed_count bigint NOT NULL,
    actor text NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT job_run_history_cleanup_retention_check CHECK (retention_days > 0),
    CONSTRAINT job_run_history_cleanup_count_check CHECK (removed_count >= 0),
    CONSTRAINT job_run_history_cleanup_actor_check CHECK (char_length(actor) BETWEEN 1 AND 200)
);
