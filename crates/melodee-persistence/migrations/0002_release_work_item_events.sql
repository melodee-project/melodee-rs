-- Append-only work-item transition history (SPEC.md section 11;
-- PRD-INGEST-010 operator visibility: "what happened to this Release and why").
--
-- Append-only by convention and by constraint surface: no UPDATE/DELETE paths
-- exist in the repository layer, and commonly queried values are structured
-- columns (metadata JSONB is for sparse, operator-safe extras only).

CREATE TABLE release_work_item_events (
    id               bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    work_item_id     uuid NOT NULL REFERENCES release_work_items (id) ON DELETE CASCADE,
    event_type       text NOT NULL,
    from_state       text NULL,
    to_state         text NULL,
    reason_code      text NULL,
    worker_id        text NULL,
    correlation_id   uuid NOT NULL,
    created_at       timestamptz NOT NULL,
    metadata         jsonb NULL,

    CONSTRAINT release_work_item_events_type_check CHECK (
        event_type IN (
            'created', 'claimed', 'lease_renewed', 'lease_released',
            'lease_expired_recovered', 'transitioned', 'failed',
            'retry_scheduled', 'completed'
        )
    ),
    CONSTRAINT release_work_item_events_state_check CHECK (
        (from_state IS NULL OR from_state IN (
            'discovered', 'settling', 'ready', 'inspecting', 'normalizing',
            'validating', 'needs_review', 'staged', 'publishing', 'published',
            'cataloging', 'cataloged', 'indexing', 'indexed', 'failed',
            'quarantined', 'ignored', 'deleted'
        ))
        AND
        (to_state IS NULL OR to_state IN (
            'discovered', 'settling', 'ready', 'inspecting', 'normalizing',
            'validating', 'needs_review', 'staged', 'publishing', 'published',
            'cataloging', 'cataloged', 'indexing', 'indexed', 'failed',
            'quarantined', 'ignored', 'deleted'
        ))
    )
);

CREATE INDEX release_work_item_events_item_idx
    ON release_work_item_events (work_item_id, created_at, id);

CREATE INDEX release_work_item_events_created_idx
    ON release_work_item_events (created_at);
