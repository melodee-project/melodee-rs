-- Durable evidence for the proposed same-filesystem custody handoff. This
-- table alone does not authorize moving or deleting a source; the worker must
-- prove the held occurrence and content before advancing each state.
CREATE TABLE inbound_custody_claims (
    -- Non-owning historical ID avoids blocking a later legitimate deletion
    -- after filesystem cleanup; pending/orphan claims require explicit repair.
    work_item_id uuid PRIMARY KEY,
    source_library_id uuid,
    source_path text NOT NULL,
    custody_name text NOT NULL UNIQUE CHECK (custody_name ~ '^[.]melodee-custody-[0-9a-f-]{36}$'),
    source_root_generation text NOT NULL CHECK (source_root_generation <> ''),
    source_generation text NOT NULL,
    preclaim_tree_hash text NOT NULL CHECK (preclaim_tree_hash ~ '^v2:[0-9a-f]{64}$'),
    preclaim_legacy_tree_hash text NOT NULL CHECK (preclaim_legacy_tree_hash ~ '^[0-9a-f]{64}$'),
    status text NOT NULL CHECK (status IN ('intent','held','replacement_preserved','replacement_restored','staged','cleaned')),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);

CREATE INDEX inbound_custody_claims_pending_idx
    ON inbound_custody_claims (status, updated_at, work_item_id)
    WHERE status <> 'cleaned';
