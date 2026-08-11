-- Phase 6 rebuildable search projection lifecycle (ADR-0121).
--
-- PostgreSQL remains authoritative. This migration is additive: it does not
-- rewrite catalog rows, media references, manifests, or existing outbox work.
-- Binary rollback leaves these tables in place. A schema rollback uses a
-- verified pre-0009 backup after draining search workers; Tantivy generations
-- are disposable and are rebuilt after forward migration.

ALTER TABLE catalog_projection_outbox
    ADD COLUMN failure_kind text NULL,
    ADD COLUMN failed_at timestamptz NULL;

ALTER TABLE catalog_projection_outbox
    ADD CONSTRAINT catalog_projection_outbox_failure_check CHECK (
        (failure_kind IS NULL AND failed_at IS NULL)
        OR (btrim(failure_kind) <> '' AND failed_at IS NOT NULL
            AND processed_at IS NULL AND lease_owner IS NULL AND lease_expires_at IS NULL)
    );

CREATE INDEX catalog_projection_outbox_search_claim_idx
    ON catalog_projection_outbox (projection_name, available_at, id)
    WHERE processed_at IS NULL AND failed_at IS NULL;

-- One writer lease and diagnostic high-water mark per projection. The outbox
-- rows themselves remain the correctness boundary; this value is not used to
-- skip unprocessed rows.
CREATE TABLE search_projection_checkpoint (
    projection_name text PRIMARY KEY,
    last_processed_outbox_id bigint NOT NULL DEFAULT 0,
    lease_owner text NULL,
    lease_expires_at timestamptz NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT search_projection_checkpoint_name_check CHECK (btrim(projection_name) <> ''),
    CONSTRAINT search_projection_checkpoint_high_water_check CHECK (
        last_processed_outbox_id >= 0
    ),
    CONSTRAINT search_projection_checkpoint_lease_check CHECK (
        (lease_owner IS NULL AND lease_expires_at IS NULL)
        OR (btrim(lease_owner) <> '' AND lease_expires_at IS NOT NULL)
    )
);

-- A generation is an owned Tantivy directory. Status changes are fenced by
-- owner_lease while building/validating. Active/retired generations have no
-- writer lease.
CREATE TABLE search_generations (
    generation_id uuid PRIMARY KEY,
    search_root text NOT NULL,
    schema_version integer NOT NULL,
    status text NOT NULL,
    created_at timestamptz NOT NULL,
    validated_at timestamptz NULL,
    activated_at timestamptz NULL,
    retired_at timestamptz NULL,
    removed_at timestamptz NULL,
    document_counts jsonb NOT NULL DEFAULT '{"artists":0,"releases":0,"tracks":0}'::jsonb,
    source_high_water_mark bigint NOT NULL DEFAULT 0,
    caught_up_outbox_id bigint NOT NULL DEFAULT 0,
    manifest_hash text NULL,
    owner_lease text NULL,
    owner_lease_expires_at timestamptz NULL,
    failure_kind text NULL,
    error text NULL,

    CONSTRAINT search_generations_id_v7_check CHECK (
        COALESCE(uuid_extract_version(generation_id) = 7, false)
    ),
    CONSTRAINT search_generations_root_check CHECK (btrim(search_root) <> ''),
    CONSTRAINT search_generations_schema_check CHECK (schema_version > 0),
    CONSTRAINT search_generations_status_check CHECK (
        status IN ('building', 'validated', 'active', 'retired', 'failed')
    ),
    CONSTRAINT search_generations_high_water_check CHECK (
        source_high_water_mark >= 0 AND caught_up_outbox_id >= source_high_water_mark
    ),
    CONSTRAINT search_generations_counts_check CHECK (
        jsonb_typeof(document_counts) = 'object'
        AND document_counts ?& ARRAY['artists', 'releases', 'tracks']
        AND document_counts - 'artists' - 'releases' - 'tracks' = '{}'::jsonb
        AND jsonb_typeof(document_counts -> 'artists') = 'number'
        AND jsonb_typeof(document_counts -> 'releases') = 'number'
        AND jsonb_typeof(document_counts -> 'tracks') = 'number'
        AND (document_counts ->> 'artists') ~ '^(0|[1-9][0-9]*)$'
        AND (document_counts ->> 'releases') ~ '^(0|[1-9][0-9]*)$'
        AND (document_counts ->> 'tracks') ~ '^(0|[1-9][0-9]*)$'
        AND (document_counts ->> 'artists')::numeric <= 9223372036854775807
        AND (document_counts ->> 'releases')::numeric <= 9223372036854775807
        AND (document_counts ->> 'tracks')::numeric <= 9223372036854775807
    ),
    CONSTRAINT search_generations_manifest_check CHECK (
        manifest_hash IS NULL OR manifest_hash ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT search_generations_lease_check CHECK (
        (owner_lease IS NULL AND owner_lease_expires_at IS NULL)
        OR (status IN ('building', 'validated')
            AND btrim(owner_lease) <> '' AND owner_lease_expires_at IS NOT NULL)
    ),
    CONSTRAINT search_generations_failure_check CHECK (
        (status <> 'failed' AND failure_kind IS NULL AND error IS NULL)
        OR (status = 'failed' AND btrim(failure_kind) <> '' AND btrim(error) <> '')
    ),
    CONSTRAINT search_generations_timestamps_check CHECK (
        (validated_at IS NULL OR status IN ('validated', 'active', 'retired'))
        AND (activated_at IS NULL OR status IN ('active', 'retired'))
        AND (retired_at IS NULL OR status = 'retired')
        AND (removed_at IS NULL OR (status = 'retired' AND retired_at IS NOT NULL))
    ),
    UNIQUE (generation_id, search_root)
);

CREATE INDEX search_generations_root_status_idx
    ON search_generations (search_root, status, created_at DESC, generation_id DESC);

-- Durable rebuild request/status. A partial unique index permits one live job
-- per search root while retaining immutable historical evidence.
CREATE TABLE search_rebuild_jobs (
    job_id uuid PRIMARY KEY,
    search_root text NOT NULL,
    requested_at timestamptz NOT NULL,
    requested_by text NOT NULL,
    status text NOT NULL,
    generation_id uuid NULL REFERENCES search_generations (generation_id) ON DELETE RESTRICT,
    requested_high_water_mark bigint NULL,
    caught_up_at timestamptz NULL,
    available_at timestamptz NOT NULL,
    lease_owner text NULL,
    lease_expires_at timestamptz NULL,
    attempt_count integer NOT NULL DEFAULT 0,
    last_error text NULL,
    failure_kind text NULL,
    error text NULL,
    finished_at timestamptz NULL,

    CONSTRAINT search_rebuild_jobs_id_v7_check CHECK (
        COALESCE(uuid_extract_version(job_id) = 7, false)
    ),
    CONSTRAINT search_rebuild_jobs_root_check CHECK (btrim(search_root) <> ''),
    CONSTRAINT search_rebuild_jobs_by_check CHECK (btrim(requested_by) <> ''),
    CONSTRAINT search_rebuild_jobs_status_check CHECK (
        status IN ('requested', 'running', 'validated', 'activated', 'failed')
    ),
    CONSTRAINT search_rebuild_jobs_high_water_check CHECK (
        requested_high_water_mark IS NULL OR requested_high_water_mark >= 0
    ),
    CONSTRAINT search_rebuild_jobs_attempt_check CHECK (attempt_count >= 0),
    CONSTRAINT search_rebuild_jobs_lease_check CHECK (
        (lease_owner IS NULL AND lease_expires_at IS NULL)
        OR (status IN ('running', 'validated')
            AND btrim(lease_owner) <> '' AND lease_expires_at IS NOT NULL)
    ),
    CONSTRAINT search_rebuild_jobs_failure_check CHECK (
        (status <> 'failed' AND failure_kind IS NULL AND error IS NULL)
        OR (status = 'failed' AND btrim(failure_kind) <> '' AND btrim(error) <> '')
    ),
    CONSTRAINT search_rebuild_jobs_finished_check CHECK (
        (status IN ('activated', 'failed') AND finished_at IS NOT NULL)
        OR (status NOT IN ('activated', 'failed') AND finished_at IS NULL)
    )
);

CREATE UNIQUE INDEX search_rebuild_jobs_one_live_root_idx
    ON search_rebuild_jobs (search_root)
    WHERE status IN ('requested', 'running', 'validated');

CREATE INDEX search_rebuild_jobs_claim_idx
    ON search_rebuild_jobs (available_at, requested_at, job_id)
    WHERE status IN ('requested', 'running', 'validated');

-- Exactly one active generation per root. The composite FK prevents a pointer
-- from crossing roots. Repository activation also verifies status='active' in
-- the same transaction.
CREATE TABLE search_active_generation (
    search_root text PRIMARY KEY,
    generation_id uuid NOT NULL,
    activated_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT search_active_generation_root_check CHECK (btrim(search_root) <> ''),
    CONSTRAINT search_active_generation_generation_fk FOREIGN KEY (generation_id, search_root)
        REFERENCES search_generations (generation_id, search_root) ON DELETE RESTRICT
);

CREATE FUNCTION validate_search_active_generation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM search_generations generation
        WHERE generation.generation_id = NEW.generation_id
          AND generation.search_root = NEW.search_root
          AND generation.status = 'active'
          AND generation.activated_at = NEW.activated_at
    ) THEN
        RAISE EXCEPTION 'active search generation is not an active generation for this root'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER search_active_generation_matches_status
AFTER INSERT OR UPDATE ON search_active_generation
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION validate_search_active_generation();
