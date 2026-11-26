-- PERF-V001 / RM-05: commit-safe search generation activation replay state.
--
-- `catalog_projection_outbox.id` is allocated before commit, so the highest
-- committed id is not a commit-order watermark. A rebuild snapshot (REPEATABLE
-- READ) can exclude transaction A's low id while including thousands of later
-- ids; catch-up advances past A; A commits; the ordinary worker acknowledges A
-- against the *old* generation; and activation then believes it is caught up.
-- A's mutation is omitted from the new generation permanently.
--
-- The corrected protocol records, inside the snapshot, exactly which outbox
-- rows the snapshot could see. Every other row is a delivery obligation of the
-- new generation regardless of its id, its processed_at, or whether the
-- ordinary worker already acknowledged it against the previous generation.
-- `search_rebuild_applied_outbox` records the replay that the new generation
-- has durably applied. Activation takes an ACCESS EXCLUSIVE lock on
-- catalog_projection_outbox (which waits for every in-flight transaction that
-- inserted an outbox row), then requires that no delivery obligation remains.
--
-- Additive only; rows are removed with their generation by ON DELETE CASCADE.

CREATE TABLE search_rebuild_snapshot_outbox (
    generation_id uuid NOT NULL
        REFERENCES search_generations (generation_id) ON DELETE CASCADE,
    outbox_id bigint NOT NULL,

    CONSTRAINT search_rebuild_snapshot_outbox_id_check CHECK (outbox_id > 0),
    PRIMARY KEY (generation_id, outbox_id)
);

CREATE TABLE search_rebuild_applied_outbox (
    generation_id uuid NOT NULL
        REFERENCES search_generations (generation_id) ON DELETE CASCADE,
    outbox_id bigint NOT NULL,
    applied_at timestamptz NOT NULL,

    CONSTRAINT search_rebuild_applied_outbox_id_check CHECK (outbox_id > 0),
    PRIMARY KEY (generation_id, outbox_id)
);
