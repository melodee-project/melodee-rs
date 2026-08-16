-- ADR-0145: a source path is a transient inbox occurrence, not permanent
-- Release identity. PostgreSQL records each durable filesystem handoff.

ALTER TABLE release_work_items
    ADD COLUMN inbound_active boolean NOT NULL DEFAULT true;

DROP INDEX release_work_items_source_identity_uniq;

CREATE UNIQUE INDEX release_work_items_active_inbound_identity_uniq
    ON release_work_items ((COALESCE(source_library_id::text, '__default__') || '|' || source_path))
    WHERE inbound_active;

CREATE TABLE release_work_item_staging_handoffs (
    work_item_id uuid PRIMARY KEY REFERENCES release_work_items (id) ON DELETE CASCADE,
    staging_relative_path text NOT NULL,
    source_generation text NOT NULL,
    status text NOT NULL,
    method text NULL,
    tree_hash text NULL,
    intended_at timestamptz NOT NULL,
    staged_at timestamptz NULL,
    inbound_removed_at timestamptz NULL,

    CONSTRAINT release_work_item_staging_handoffs_status_check CHECK (
        status IN ('intent', 'staged', 'inbound_removed')
    ),
    CONSTRAINT release_work_item_staging_handoffs_method_check CHECK (
        method IS NULL OR method IN ('rename', 'verified_copy')
    ),
    CONSTRAINT release_work_item_staging_handoffs_receipt_check CHECK (
        (status = 'intent' AND method IS NULL AND tree_hash IS NULL AND staged_at IS NULL)
        OR (status IN ('staged', 'inbound_removed') AND method IS NOT NULL
            AND tree_hash IS NOT NULL AND staged_at IS NOT NULL)
    )
);

CREATE INDEX release_work_item_staging_handoffs_status_idx
    ON release_work_item_staging_handoffs (status, intended_at);
