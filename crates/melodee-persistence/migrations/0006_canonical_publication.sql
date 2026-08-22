-- Durable canonical-publication intent and convergence evidence
-- (ADR-0116). Authorization and plans are insert-only through the repository;
-- completion is a separate immutable receipt and progress is append-only.

CREATE TABLE release_publication_authorizations (
    authorization_id uuid PRIMARY KEY,
    work_item_id uuid NOT NULL UNIQUE REFERENCES release_work_items (id) ON DELETE RESTRICT,
    release_id uuid NOT NULL UNIQUE,
    authorization_name text NOT NULL,
    authorization_version integer NOT NULL,
    validation_profile_name text NOT NULL,
    validation_profile_version integer NOT NULL,
    validation_outcome text NOT NULL,
    validation_payload jsonb NOT NULL,
    source_disposition text NOT NULL,
    authorized_at timestamptz NOT NULL,

    CONSTRAINT release_publication_authorizations_phase3_check CHECK (
        authorization_name = 'phase3.structural-publication'
        AND authorization_version = 1
        AND validation_profile_name <> ''
        AND validation_profile_version > 0
        AND validation_outcome = 'passed'
        AND source_disposition = 'retained'
    ),
    UNIQUE (authorization_id, work_item_id, release_id)
);

CREATE TABLE release_publication_plans (
    work_item_id uuid PRIMARY KEY REFERENCES release_work_items (id) ON DELETE RESTRICT,
    authorization_id uuid NOT NULL UNIQUE,
    release_id uuid NOT NULL UNIQUE,
    layout_version integer NOT NULL,
    input_manifest_hash text NOT NULL,
    target_relative_path text NOT NULL UNIQUE,
    manifest_relative_path text NOT NULL UNIQUE,
    plan_hash text NOT NULL UNIQUE,
    plan_payload jsonb NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT release_publication_plans_authorization_fk
        FOREIGN KEY (authorization_id, work_item_id, release_id)
        REFERENCES release_publication_authorizations (authorization_id, work_item_id, release_id)
        ON DELETE RESTRICT,
    CONSTRAINT release_publication_plans_layout_check CHECK (layout_version = 1),
    CONSTRAINT release_publication_plans_input_hash_check CHECK (
        input_manifest_hash ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT release_publication_plans_hash_check CHECK (plan_hash ~ '^[0-9a-f]{64}$'),
    CONSTRAINT release_publication_plans_manifest_path_check CHECK (
        manifest_relative_path = target_relative_path || '/manifest.json'
    ),
    CONSTRAINT release_publication_plans_target_path_check CHECK (
        target_relative_path ~ '^releases/[0-9a-f]{2}/[0-9a-f]{2}/rel_[0-9a-f]{32}$'
    ),
    UNIQUE (work_item_id, release_id, plan_hash)
);

CREATE TABLE release_publication_journal (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    work_item_id uuid NOT NULL REFERENCES release_publication_plans (work_item_id) ON DELETE RESTRICT,
    journal_key text NOT NULL,
    step text NOT NULL,
    asset_path text NULL,
    size_bytes bigint NULL,
    byte_hash text NULL,
    worker_id text NOT NULL,
    metadata jsonb NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT release_publication_journal_key_check CHECK (journal_key <> ''),
    CONSTRAINT release_publication_journal_step_check CHECK (
        step IN (
            'incoming_prepared', 'asset_copied', 'bundle_sealed', 'finalized',
            'final_adopted', 'target_conflict', 'failed'
        )
    ),
    CONSTRAINT release_publication_journal_size_check CHECK (size_bytes IS NULL OR size_bytes >= 0),
    CONSTRAINT release_publication_journal_hash_check CHECK (
        byte_hash IS NULL OR byte_hash ~ '^[0-9a-f]{64}$'
    ),
    UNIQUE (work_item_id, journal_key)
);

CREATE INDEX release_publication_journal_item_idx
    ON release_publication_journal (work_item_id, created_at, id);

CREATE TABLE release_publication_receipts (
    work_item_id uuid PRIMARY KEY REFERENCES release_publication_plans (work_item_id) ON DELETE RESTRICT,
    release_id uuid NOT NULL UNIQUE,
    plan_hash text NOT NULL UNIQUE,
    sealed_manifest_hash text NOT NULL,
    source_disposition text NOT NULL,
    published_at timestamptz NOT NULL,

    CONSTRAINT release_publication_receipts_plan_fk
        FOREIGN KEY (work_item_id, release_id, plan_hash)
        REFERENCES release_publication_plans (work_item_id, release_id, plan_hash)
        ON DELETE RESTRICT,

    CONSTRAINT release_publication_receipts_hash_check CHECK (
        plan_hash ~ '^[0-9a-f]{64}$' AND sealed_manifest_hash ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT release_publication_receipts_disposition_check CHECK (
        source_disposition = 'retained'
    )
);

-- Staged intent is unleased; publication acquires a new fenced lease. Published
-- work is likewise never left behind an active worker lease.
UPDATE release_work_items
SET lease_owner = NULL, lease_acquired_at = NULL, lease_expires_at = NULL
WHERE state IN ('validating', 'staged', 'published');

ALTER TABLE release_work_items
    ADD CONSTRAINT release_work_items_publication_lease_check CHECK (
        state NOT IN ('staged', 'published')
        OR (lease_owner IS NULL AND lease_acquired_at IS NULL AND lease_expires_at IS NULL)
    );

CREATE INDEX release_work_items_validation_claim_idx
    ON release_work_items (updated_at, id)
    WHERE state = 'validating';

CREATE INDEX release_work_items_publication_claim_idx
    ON release_work_items (updated_at, id)
    WHERE state IN ('staged', 'publishing');
