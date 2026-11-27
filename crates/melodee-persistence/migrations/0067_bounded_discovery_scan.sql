-- Durable per-source discovery with bounded per-work-item partition markers.
-- Each partition is replayable and its observation bit can be reset without
-- losing aliases observed in previously completed partitions.
CREATE TABLE ingestion_discovery_scan (
    scope_key text PRIMARY KEY,
    generation uuid NOT NULL,
    owner uuid NOT NULL,
    lease_expires_at timestamptz NOT NULL,
    birth_cutoff timestamptz NOT NULL,
    partition smallint NOT NULL DEFAULT 0 CHECK (partition BETWEEN 0 AND 16),
    phase text NOT NULL DEFAULT 'collect' CHECK (phase IN ('collect', 'clear', 'replay', 'missing')),
    phase_started boolean NOT NULL DEFAULT false,
    initial_pass_complete boolean NOT NULL DEFAULT false,
    restart_observations boolean NOT NULL DEFAULT false,
    recovery_reset boolean NOT NULL DEFAULT false,
    missing_resume boolean NOT NULL DEFAULT false,
    missing_revalidation boolean NOT NULL DEFAULT false,
    missing_retry boolean NOT NULL DEFAULT false,
    retry_mask integer NOT NULL DEFAULT 0,
    clear_after uuid NULL,
    CHECK (NOT recovery_reset OR (phase = 'clear' AND partition = 0))
);

CREATE INDEX ingestion_discovery_scan_expiry_idx ON ingestion_discovery_scan (lease_expires_at);

ALTER TABLE release_work_items
    ADD COLUMN discovery_birth_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    ADD COLUMN discovery_path_format smallint NOT NULL DEFAULT 0 CHECK (discovery_path_format IN (0, 1)),
    ADD COLUMN discovery_scan_generation uuid NULL,
    ADD COLUMN discovery_missing_generation uuid NULL,
    ADD COLUMN discovery_missing_retry_generation uuid NULL,
    ADD COLUMN discovery_seen_mask integer NOT NULL DEFAULT 0,
    ADD COLUMN discovery_done_mask integer NOT NULL DEFAULT 0;

-- The stable default above lets PostgreSQL represent the migration-time value
-- for pre-existing rows without rewriting the table. New occurrences need a
-- true insertion-time cutoff, including within long-running transactions.
ALTER TABLE release_work_items
    ALTER COLUMN discovery_birth_at SET DEFAULT clock_timestamp(),
    ALTER COLUMN discovery_path_format SET DEFAULT 1;

CREATE INDEX release_work_items_discovery_scope_idx
    ON release_work_items (source_library_id, id)
    WHERE inbound_active AND state IN ('discovered', 'settling', 'ready');

CREATE INDEX release_work_items_discovery_generation_idx
    ON release_work_items (source_library_id, discovery_scan_generation, id)
    WHERE discovery_scan_generation IS NOT NULL;

-- A path may exceed PostgreSQL's B-tree entry limit. The digest narrows an
-- exact text comparison; collisions never establish identity or uniqueness.
DROP INDEX release_work_items_active_inbound_identity_uniq;
CREATE FUNCTION ingest_occurrence_hash(path text) RETURNS bytea
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    RETURN sha256(convert_to(path, 'UTF8'));
CREATE INDEX release_work_items_discovery_path_hash_idx
    ON release_work_items (source_library_id, ingest_occurrence_hash(source_path));

CREATE FUNCTION lock_inbound_occurrence(source_id uuid, path text) RETURNS void
    LANGUAGE sql VOLATILE
    RETURN pg_advisory_xact_lock(1869834357, hashtext(COALESCE(source_id::text, '__default__') || '|' || path));

CREATE FUNCTION enforce_inbound_occurrence_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.inbound_active THEN
        PERFORM lock_inbound_occurrence(NEW.source_library_id, NEW.source_path);
        IF EXISTS (SELECT 1 FROM release_work_items item
            WHERE item.source_library_id IS NOT DISTINCT FROM NEW.source_library_id
              AND ingest_occurrence_hash(item.source_path)=ingest_occurrence_hash(NEW.source_path)
              AND item.source_path=NEW.source_path AND item.inbound_active AND item.id<>NEW.id
              AND (strpos(NEW.source_path,chr(65533))=0 OR item.discovery_path_format=NEW.discovery_path_format)) THEN
            RAISE EXCEPTION 'active inbound occurrence already exists' USING ERRCODE='23505',
                CONSTRAINT='release_work_items_active_inbound_identity_uniq';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER release_work_items_active_inbound_identity_guard
    BEFORE INSERT OR UPDATE OF source_library_id, source_path, inbound_active, discovery_path_format ON release_work_items
    FOR EACH ROW EXECUTE FUNCTION enforce_inbound_occurrence_identity();

-- An operator decision binds an ambiguous historical row to reviewed,
-- receipt-verified staging evidence without deleting either work history.
CREATE TABLE ingest_legacy_path_resolutions (
    -- Non-owning historical IDs: the binding transaction locks and verifies
    -- both rows; audit evidence survives later legitimate work-item deletion.
    legacy_work_item_id uuid PRIMARY KEY,
    reviewed_work_item_id uuid NOT NULL,
    receipt_tree_hash text NOT NULL CHECK (receipt_tree_hash ~ '^[0-9a-f]{64}$'),
    actor text NOT NULL CHECK (length(btrim(actor)) BETWEEN 1 AND 256),
    reason text NOT NULL CHECK (length(btrim(reason)) BETWEEN 1 AND 4096),
    resolved_at timestamptz NOT NULL,
    CHECK (legacy_work_item_id <> reviewed_work_item_id)
);
