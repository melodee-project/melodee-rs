-- Feature-Completion Phase 4 / ADR-0157 + ADR-0164.
-- Additive and rollback-safe: dropping these audit rows does not remove or
-- rewrite media. A rollback restores a verified pre-0037 database backup.

CREATE TABLE metadata_edit_audits (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases(id) ON DELETE RESTRICT,
    operation_name text NOT NULL,
    operation_params jsonb NOT NULL,
    affected_files integer NOT NULL,
    field_deltas jsonb NOT NULL,
    applied_by text NOT NULL,
    status text NOT NULL,
    recovery_evidence jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL,
    completed_at timestamptz NULL,
    CONSTRAINT metadata_edit_audits_id_v7 CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT metadata_edit_audits_operation_check CHECK (
        btrim(operation_name) <> '' AND char_length(operation_name) <= 100
    ),
    CONSTRAINT metadata_edit_audits_files_check CHECK (affected_files >= 0),
    CONSTRAINT metadata_edit_audits_status_check CHECK (
        status IN ('preview', 'applying', 'success', 'failed', 'rolled_back', 'reverted')
    ),
    CONSTRAINT metadata_edit_audits_actor_check CHECK (
        btrim(applied_by) <> '' AND char_length(applied_by) <= 200
    ),
    CONSTRAINT metadata_edit_audits_time_check CHECK (completed_at IS NULL OR completed_at >= created_at)
);

CREATE INDEX metadata_edit_audits_release_idx
    ON metadata_edit_audits (release_id, created_at DESC, id DESC);

CREATE INDEX metadata_edit_audits_status_idx
    ON metadata_edit_audits (status, created_at DESC);
