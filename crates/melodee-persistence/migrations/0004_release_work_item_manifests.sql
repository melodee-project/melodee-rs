-- Phase 1C: durable versioned manifest publication and inspection evidence.

CREATE TABLE release_work_item_manifests (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    work_item_id uuid NOT NULL REFERENCES release_work_items (id) ON DELETE CASCADE,
    schema_version integer NOT NULL,
    manifest_hash text NOT NULL,
    source_snapshot_id bigint NULL,
    source_signature text NULL,
    manifest_payload jsonb NOT NULL,
    ffprobe_path text NULL,
    ffprobe_version text NULL,
    tool_metadata jsonb NULL,
    source_stable boolean NOT NULL,
    generation_duration_ms bigint NOT NULL,
    generated_at timestamptz NOT NULL,

    CONSTRAINT release_work_item_manifests_schema_version_check CHECK (schema_version > 0),
    CONSTRAINT release_work_item_manifests_generation_duration_ms_check CHECK (generation_duration_ms >= 0)
);

-- At most one durable manifest row exists per work item.
CREATE UNIQUE INDEX release_work_item_manifests_work_item_uniq
    ON release_work_item_manifests (work_item_id);
CREATE INDEX release_work_item_manifests_created_at_idx
    ON release_work_item_manifests (generated_at, id);

-- Track manifest evolution for future upgrades while avoiding large payload joins at state transitions.
CREATE INDEX release_work_item_manifests_manifest_hash_idx
    ON release_work_item_manifests (manifest_hash);
