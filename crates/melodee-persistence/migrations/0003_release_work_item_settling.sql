-- Phase 1B: stable settling evidence and source-identity deduplication
--
-- Settling is a source truth operation; snapshots must be durable and inspectable.
-- Unique source identity allows concurrent reconcilers to upsert the same candidate
-- safely without duplicate work items.

ALTER TABLE release_work_items
    ADD COLUMN latest_settling_snapshot_id bigint NULL,
    ADD COLUMN stable_since timestamptz NULL,
    ADD COLUMN last_observed_at timestamptz NULL;

CREATE UNIQUE INDEX release_work_items_source_identity_uniq
    ON release_work_items ((COALESCE(source_library_id::text, '__default__') || '|' || source_path));

CREATE TABLE release_work_item_settling_snapshots (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    work_item_id uuid NOT NULL REFERENCES release_work_items (id) ON DELETE CASCADE,
    observed_at timestamptz NOT NULL,
    signature text NOT NULL,
    file_count integer NOT NULL,
    total_bytes bigint NOT NULL,
    ready_marker_seen boolean NOT NULL,
    bypassed_settling boolean NOT NULL,

    CONSTRAINT release_work_item_settling_snapshots_file_count_check CHECK (file_count >= 0),
    CONSTRAINT release_work_item_settling_snapshots_total_bytes_check CHECK (total_bytes >= 0)
);

CREATE TABLE release_work_item_settling_snapshot_files (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    snapshot_id bigint NOT NULL REFERENCES release_work_item_settling_snapshots (id) ON DELETE CASCADE,
    relative_path text NOT NULL,
    file_type text NOT NULL,
    file_size bigint NOT NULL,
    modified_at timestamptz NOT NULL,

    CONSTRAINT release_work_item_settling_snapshot_files_size_check CHECK (file_size >= 0),
    UNIQUE (snapshot_id, relative_path)
);

CREATE INDEX release_work_item_settling_snapshots_work_item_idx
    ON release_work_item_settling_snapshots (work_item_id, observed_at DESC, id DESC);

CREATE INDEX release_work_item_settling_snapshot_files_snapshot_idx
    ON release_work_item_settling_snapshot_files (snapshot_id, relative_path);
