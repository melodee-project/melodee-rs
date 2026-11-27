-- Protected-stage attempts require a fresh random epoch for every work-item
-- claim. Existing active leases remain valid for ordinary work but cannot
-- reserve protected attempts until they are reclaimed after maintenance.
ALTER TABLE release_work_items ADD COLUMN protected_attempt_lease_epoch uuid;
-- Once protected custody starts, worker-only APIs cannot authorize media operations.
ALTER TABLE release_work_items ADD COLUMN protected_custody_required boolean NOT NULL DEFAULT false;
CREATE FUNCTION protect_custody_mode() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.protected_custody_required AND NOT NEW.protected_custody_required THEN
        RAISE EXCEPTION 'protected custody mode is durable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER protect_custody_mode BEFORE UPDATE ON release_work_items
    FOR EACH ROW EXECUTE FUNCTION protect_custody_mode();

-- The independent cursor advances even over busy occurrences; it wraps when
-- the bounded keyset reaches the end, so an unavailable source cannot starve peers.
CREATE TABLE protected_inspection_recovery_cursor (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    last_expiry timestamptz,
    last_id uuid,
    CHECK ((last_expiry IS NULL) = (last_id IS NULL))
);
INSERT INTO protected_inspection_recovery_cursor(singleton) VALUES(true);
CREATE INDEX protected_inspection_recovery_idx ON release_work_items
    (COALESCE(lease_expires_at, '0001-01-01 00:00:00+00'::timestamptz),id)
    WHERE protected_custody_required AND state='inspecting';

-- Bind a completed unpublished-review deletion to its reservation for
-- idempotent lost-ack replay; no media payload is stored here.
ALTER TABLE review_deletion_authorizations ADD COLUMN admin_reservation_id uuid;
ALTER TABLE review_deletion_authorizations ADD COLUMN admin_reservation_token uuid;
ALTER TABLE review_deletion_authorizations ADD COLUMN admin_reservation_owner text;
ALTER TABLE review_deletion_authorizations ADD CONSTRAINT review_deletion_reservation_all_or_none CHECK (
    (admin_reservation_id IS NULL AND admin_reservation_token IS NULL AND admin_reservation_owner IS NULL) OR
    (admin_reservation_id IS NOT NULL AND admin_reservation_token IS NOT NULL AND admin_reservation_owner IS NOT NULL));

-- Bounded metadata-only spool for a prospective streamed custody digest.
-- Rows contain cryptographic digests, not file payloads. Incomplete captures
-- are never receipts and require an explicit, fenced completion protocol.
ALTER TABLE release_work_item_staging_handoffs
    ADD COLUMN retained_source boolean NOT NULL DEFAULT false;
-- Non-owning work-item identity avoids a filesystem-before-FK-delete failure.
-- Rows are durable reobservation obligations, not a substitute for source bytes.
CREATE TABLE inbound_retained_sources (
    work_item_id uuid PRIMARY KEY,
    source_library_id uuid,
    baseline_id uuid,
    baseline_cursor_file bigint NOT NULL DEFAULT 0 CHECK (baseline_cursor_file>=0),
    baseline_cursor_chunk bigint NOT NULL DEFAULT -1 CHECK (baseline_cursor_chunk>=-1),
    baseline_cursor_revision bigint NOT NULL DEFAULT 0 CHECK (baseline_cursor_revision>=0),
    baseline_completed_passes bigint NOT NULL DEFAULT 0 CHECK (baseline_completed_passes>=0),
    baseline_change_pending boolean NOT NULL DEFAULT false,
    baseline_directory_turn boolean NOT NULL DEFAULT false,
    baseline_file_due_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    baseline_directory_blocked_until timestamptz,
    baseline_directory_unavailable_reason text CHECK (baseline_directory_unavailable_reason IS NULL OR baseline_directory_unavailable_reason='unsupported'),
    baseline_unreadable_reason text CHECK (baseline_unreadable_reason IS NULL OR baseline_unreadable_reason IN ('missing','unreadable')),
    baseline_unreadable_count integer NOT NULL DEFAULT 0 CHECK (baseline_unreadable_count>=0),
    source_path text NOT NULL CHECK (octet_length(source_path) BETWEEN 1 AND 262200),
    source_path_format smallint NOT NULL DEFAULT 1 CHECK (source_path_format IN (0,1)),
    source_generation text NOT NULL CHECK (octet_length(source_generation) BETWEEN 1 AND 512),
    content_digest text NOT NULL CHECK (content_digest ~ '^v3:[0-9a-f]{64}$'),
    next_reobserve_at timestamptz NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','superseded')),
    superseded_by uuid,
    lease_owner text CHECK (lease_owner IS NULL OR octet_length(lease_owner) BETWEEN 1 AND 512),
    lease_token uuid,
    lease_expires_at timestamptz,
    retry_count integer NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    last_retry_reason text CHECK (last_retry_reason IS NULL OR last_retry_reason IN ('missing','unreadable','unstable','capacity','ambiguous','awaiting_intake','other')),
    completed_lease_token uuid,
    completed_session_id uuid,
    completed_epoch uuid,
    completed_digest text CHECK (completed_digest IS NULL OR completed_digest ~ '^v3:[0-9a-f]{64}$'),
    completed_generation text CHECK (completed_generation IS NULL OR octet_length(completed_generation) BETWEEN 1 AND 512),
    completed_outcome text CHECK (completed_outcome IS NULL OR completed_outcome IN ('unchanged','superseded')),
    completed_work_item_id uuid,
    completed_created boolean,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CHECK ((lease_owner IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL) OR
           (lease_owner IS NOT NULL AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)),
    CHECK ((status='active' AND superseded_by IS NULL) OR
           (status='superseded' AND superseded_by IS NOT NULL AND lease_owner IS NULL)),
    CHECK ((completed_lease_token IS NULL AND completed_session_id IS NULL AND completed_epoch IS NULL AND completed_digest IS NULL AND completed_generation IS NULL AND completed_outcome IS NULL AND completed_work_item_id IS NULL AND completed_created IS NULL) OR
           (completed_lease_token IS NOT NULL AND completed_session_id IS NOT NULL AND completed_epoch IS NOT NULL AND completed_digest IS NOT NULL AND completed_generation IS NOT NULL AND
             ((completed_outcome='unchanged' AND completed_work_item_id IS NULL AND completed_created IS NULL) OR
              (completed_outcome='superseded' AND completed_work_item_id IS NOT NULL AND completed_created IS NOT NULL))) IS TRUE)
);
CREATE INDEX inbound_retained_sources_listing_idx ON inbound_retained_sources (work_item_id) WHERE status='active';
CREATE INDEX inbound_retained_sources_due_idx
    ON inbound_retained_sources (next_reobserve_at, work_item_id)
    WHERE status='active' AND (source_path_format=1 OR strpos(source_path,chr(65533))=0);

-- Active obligations are a durable queue with an explicit metadata budget.
-- Superseded rows are immutable historical evidence, not queued work.
CREATE TABLE inbound_retained_source_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    max_active bigint NOT NULL DEFAULT 1000000 CHECK (max_active > 0),
    max_bytes bigint NOT NULL DEFAULT 1073741824 CHECK (max_bytes > 0),
    active_count bigint NOT NULL DEFAULT 0 CHECK (active_count >= 0),
    active_bytes bigint NOT NULL DEFAULT 0 CHECK (active_bytes >= 0)
);
INSERT INTO inbound_retained_source_budget (singleton) VALUES (true);

CREATE FUNCTION inbound_retained_source_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.work_item_id <> OLD.work_item_id OR
       NEW.source_library_id IS DISTINCT FROM OLD.source_library_id OR
       NEW.baseline_id IS DISTINCT FROM OLD.baseline_id OR
       NEW.source_path_format <> OLD.source_path_format OR
       NEW.source_path COLLATE "C" <> OLD.source_path COLLATE "C" OR
       NEW.source_generation <> OLD.source_generation OR NEW.content_digest <> OLD.content_digest OR
       (OLD.status='superseded' AND NEW IS DISTINCT FROM OLD) OR
       (OLD.status='active' AND NEW.status NOT IN ('active','superseded')) THEN
        RAISE EXCEPTION 'retained source identity and historical evidence are immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER inbound_retained_source_immutable
    BEFORE UPDATE ON inbound_retained_sources
    FOR EACH ROW EXECUTE FUNCTION inbound_retained_source_guard();

CREATE FUNCTION inbound_retained_source_account() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE added bigint := 0; removed bigint := 0; item_bytes bigint;
BEGIN
    IF TG_OP='INSERT' AND NEW.status='active' THEN
        added := 1;
        item_bytes := octet_length(NEW.source_path)::bigint + octet_length(NEW.source_generation)::bigint + 512;
    ELSIF TG_OP='UPDATE' AND OLD.status='active' AND NEW.status='superseded' THEN
        removed := 1;
        item_bytes := octet_length(OLD.source_path)::bigint + octet_length(OLD.source_generation)::bigint + 512;
    ELSIF TG_OP='DELETE' AND OLD.status='active' THEN
        removed := 1;
        item_bytes := octet_length(OLD.source_path)::bigint + octet_length(OLD.source_generation)::bigint + 512;
    ELSE
        IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
    END IF;
    UPDATE inbound_retained_source_budget SET
        active_count=active_count+added-removed,
        active_bytes=active_bytes+(added-removed)*item_bytes
      WHERE singleton AND (added=0 OR (active_count < max_active AND item_bytes <= max_bytes-active_bytes));
    IF NOT FOUND THEN
        RAISE EXCEPTION 'retained source observation quota exhausted; increase limit and retry' USING ERRCODE='53400';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;
CREATE TRIGGER inbound_retained_source_account
    AFTER INSERT OR UPDATE OF status OR DELETE ON inbound_retained_sources
    FOR EACH ROW EXECUTE FUNCTION inbound_retained_source_account();
CREATE TABLE custody_hash_spool_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    max_captures bigint NOT NULL DEFAULT 1024 CHECK (max_captures > 0),
    max_entries bigint NOT NULL DEFAULT 1000000 CHECK (max_entries > 0),
    max_bytes bigint NOT NULL DEFAULT 1073741824 CHECK (max_bytes > 0),
    captures bigint NOT NULL DEFAULT 0 CHECK (captures >= 0),
    entries bigint NOT NULL DEFAULT 0 CHECK (entries >= 0),
    bytes bigint NOT NULL DEFAULT 0 CHECK (bytes >= 0)
);
INSERT INTO custody_hash_spool_budget (singleton) VALUES (true);

CREATE TABLE custody_hash_spool_captures (
    id uuid PRIMARY KEY,
    work_item_id uuid NOT NULL,
    purpose text NOT NULL CHECK (purpose IN ('source','staging','retained')),
    lease_owner text NOT NULL CHECK (octet_length(lease_owner) <= 512),
    retained_lease_token uuid,
    protected_claim_epoch uuid,
    source_generation text NOT NULL CHECK (octet_length(source_generation) BETWEEN 1 AND 512),
    target_binding text NOT NULL CHECK (octet_length(target_binding) BETWEEN 1 AND 262200),
    state text NOT NULL CHECK (state IN ('capturing','revalidating','frozen','discarding')),
    observation_epoch uuid NOT NULL,
    sealed_digest text CHECK (sealed_digest IS NULL OR sealed_digest ~ '^v3:[0-9a-f]{64}$'),
    sealed_epoch uuid,
    entry_count bigint NOT NULL DEFAULT 0 CHECK (entry_count >= 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    UNIQUE (work_item_id, purpose),
    CHECK ((sealed_digest IS NULL) = (sealed_epoch IS NULL)),
    CHECK ((purpose='retained') = (retained_lease_token IS NOT NULL)),
    CHECK (protected_claim_epoch IS NULL OR purpose<>'retained')
);

CREATE TABLE custody_hash_spool_entries (
    capture_id uuid NOT NULL REFERENCES custody_hash_spool_captures(id) ON DELETE RESTRICT,
    ordinal bigint GENERATED ALWAYS AS IDENTITY,
    path_key text NOT NULL CHECK (octet_length(path_key) BETWEEN 1 AND 262200),
    parent_key text NOT NULL CHECK (octet_length(parent_key) <= 262200),
    name_key text COLLATE "C" NOT NULL CHECK (octet_length(name_key) BETWEEN 2 AND 262144 AND octet_length(name_key)%2=0 AND translate(name_key,'0123456789abcdef','')=''),
    legacy_path_hex text COLLATE "C" NOT NULL CHECK (octet_length(legacy_path_hex) BETWEEN 2 AND 1048576 AND octet_length(legacy_path_hex)%2=0 AND translate(legacy_path_hex,'0123456789abcdef','')=''),
    is_directory boolean NOT NULL,
    legacy_length bigint NOT NULL CHECK (legacy_length >= 0),
    observed_epoch uuid NOT NULL,
    digest text COLLATE "C" NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY (capture_id, ordinal)
);
CREATE INDEX custody_hash_spool_order_idx
    ON custody_hash_spool_entries (capture_id, digest, ordinal);
CREATE INDEX custody_hash_spool_path_idx
    ON custody_hash_spool_entries (capture_id, ingest_occurrence_hash(path_key));
CREATE INDEX custody_hash_spool_siblings_idx
    ON custody_hash_spool_entries (capture_id, ingest_occurrence_hash(parent_key), left(name_key,512), ordinal);

-- The first observation is durable, but it cannot authorize a change until
-- the same retained lease independently seals a second matching epoch.
CREATE TABLE inbound_retained_recheck_sessions (
    work_item_id uuid PRIMARY KEY REFERENCES inbound_retained_sources(work_item_id) ON DELETE CASCADE,
    session_id uuid NOT NULL UNIQUE,
    lease_token uuid NOT NULL,
    capture_id uuid NOT NULL REFERENCES custody_hash_spool_captures(id) ON DELETE CASCADE,
    before_epoch uuid NOT NULL,
    before_digest text NOT NULL CHECK (before_digest ~ '^v3:[0-9a-f]{64}$'),
    before_entries bigint NOT NULL CHECK (before_entries >= 0),
    source_generation text NOT NULL CHECK (octet_length(source_generation) BETWEEN 1 AND 512),
    source_root_generation text NOT NULL CHECK (octet_length(source_root_generation) BETWEEN 1 AND 512),
    created_at timestamptz NOT NULL
);

-- This serialized session establishes before -> verified stage -> after.
-- Deleting provisional capture metadata cascades only this transient session,
-- never any work item, staging bytes, receipt, or retained-source obligation.
CREATE TABLE inbound_retained_proof_sessions (
    work_item_id uuid PRIMARY KEY,
    session_id uuid NOT NULL UNIQUE,
    lease_owner text NOT NULL,
    source_capture_id uuid NOT NULL REFERENCES custody_hash_spool_captures(id) ON DELETE CASCADE,
    protected_claim_epoch uuid,
    source_before_epoch uuid NOT NULL,
    source_before_digest text NOT NULL CHECK (source_before_digest ~ '^v3:[0-9a-f]{64}$'),
    staging_capture_id uuid REFERENCES custody_hash_spool_captures(id) ON DELETE CASCADE,
    staging_epoch uuid,
    staging_digest text CHECK (staging_digest IS NULL OR staging_digest ~ '^v3:[0-9a-f]{64}$'),
    staging_legacy_hash text CHECK (staging_legacy_hash IS NULL OR staging_legacy_hash ~ '^[0-9a-f]{64}$'),
    state text NOT NULL CHECK (state IN ('before','staging_verified','bound')),
    CHECK ((state='before' AND staging_capture_id IS NULL AND staging_epoch IS NULL AND staging_digest IS NULL AND staging_legacy_hash IS NULL) OR
           (state IN ('staging_verified','bound') AND staging_capture_id IS NOT NULL AND staging_epoch IS NOT NULL AND staging_digest IS NOT NULL AND staging_legacy_hash IS NOT NULL))
);

CREATE FUNCTION custody_hash_spool_preserve_capture() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id <> OLD.id OR NEW.work_item_id <> OLD.work_item_id OR NEW.purpose <> OLD.purpose OR
       NEW.retained_lease_token IS DISTINCT FROM OLD.retained_lease_token OR
       NEW.protected_claim_epoch IS DISTINCT FROM OLD.protected_claim_epoch OR
       NEW.source_generation COLLATE "C" <> OLD.source_generation COLLATE "C" OR
       NEW.target_binding COLLATE "C" <> OLD.target_binding COLLATE "C" THEN
        RAISE EXCEPTION 'custody hash capture binding is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER custody_hash_capture_immutable
    BEFORE UPDATE ON custody_hash_spool_captures
    FOR EACH ROW EXECUTE FUNCTION custody_hash_spool_preserve_capture();

CREATE FUNCTION custody_hash_spool_preserve_entry() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.capture_id <> OLD.capture_id OR NEW.ordinal <> OLD.ordinal OR
       NEW.path_key COLLATE "C" <> OLD.path_key COLLATE "C" OR
       NEW.parent_key COLLATE "C" <> OLD.parent_key COLLATE "C" OR
       NEW.name_key <> OLD.name_key OR NEW.legacy_path_hex <> OLD.legacy_path_hex OR NEW.is_directory <> OLD.is_directory OR
       NEW.legacy_length <> OLD.legacy_length OR NEW.digest <> OLD.digest THEN
        RAISE EXCEPTION 'custody hash entry evidence is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER custody_hash_entry_immutable
    BEFORE UPDATE ON custody_hash_spool_entries
    FOR EACH ROW EXECUTE FUNCTION custody_hash_spool_preserve_entry();

-- Serializing accounting on one singleton prevents concurrent transactions
-- from collectively exceeding quota. Cleanup deletes at most one page per
-- transaction; a capture cannot be removed while entry rows remain.
CREATE FUNCTION custody_hash_spool_account_capture() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_count bigint; maximum bigint; current_bytes bigint; maximum_bytes bigint; capture_bytes bigint;
BEGIN
    IF TG_OP = 'INSERT' THEN
        capture_bytes := octet_length(NEW.target_binding) + 1536;
        SELECT captures, max_captures, bytes, max_bytes INTO current_count, maximum, current_bytes, maximum_bytes
          FROM custody_hash_spool_budget WHERE singleton FOR UPDATE;
        IF current_count >= maximum OR capture_bytes > maximum_bytes - current_bytes THEN
            RAISE EXCEPTION 'custody hash capture quota exhausted' USING ERRCODE = '53400';
        END IF;
        UPDATE custody_hash_spool_budget SET captures = captures + 1, bytes=bytes+capture_bytes WHERE singleton;
        RETURN NEW;
    END IF;
    UPDATE custody_hash_spool_budget SET captures = captures - 1, bytes=bytes-(octet_length(OLD.target_binding)+1536) WHERE singleton;
    RETURN OLD;
END $$;
CREATE TRIGGER custody_hash_capture_account
    AFTER INSERT OR DELETE ON custody_hash_spool_captures
    FOR EACH ROW EXECUTE FUNCTION custody_hash_spool_account_capture();

CREATE FUNCTION custody_hash_spool_account_entries_inserted() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE page_count bigint; page_bytes bigint;
BEGIN
    SELECT count(*), COALESCE(sum(octet_length(path_key)::bigint + octet_length(parent_key)::bigint + octet_length(name_key)::bigint + octet_length(legacy_path_hex)::bigint + 384), 0)
      INTO page_count, page_bytes FROM custody_hash_inserted;
    UPDATE custody_hash_spool_budget SET entries=entries+page_count, bytes=bytes+page_bytes
      WHERE singleton AND page_count <= max_entries-entries AND page_bytes <= max_bytes-bytes;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'custody hash entry quota exhausted' USING ERRCODE = '53400';
    END IF;
    UPDATE custody_hash_spool_captures c SET entry_count=c.entry_count+page.entry_count
      FROM (SELECT capture_id, count(*) AS entry_count FROM custody_hash_inserted GROUP BY capture_id) page
      WHERE c.id=page.capture_id;
    RETURN NULL;
END $$;
CREATE TRIGGER custody_hash_entries_inserted
    AFTER INSERT ON custody_hash_spool_entries REFERENCING NEW TABLE AS custody_hash_inserted
    FOR EACH STATEMENT EXECUTE FUNCTION custody_hash_spool_account_entries_inserted();

CREATE FUNCTION custody_hash_spool_account_entries_deleted() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE page_count bigint; page_bytes bigint;
BEGIN
    SELECT count(*), COALESCE(sum(octet_length(path_key)::bigint + octet_length(parent_key)::bigint + octet_length(name_key)::bigint + octet_length(legacy_path_hex)::bigint + 384), 0)
      INTO page_count, page_bytes FROM custody_hash_deleted;
    UPDATE custody_hash_spool_budget SET entries=entries-page_count, bytes=bytes-page_bytes WHERE singleton;
    UPDATE custody_hash_spool_captures c SET entry_count=c.entry_count-page.entry_count
      FROM (SELECT capture_id, count(*) AS entry_count FROM custody_hash_deleted GROUP BY capture_id) page
      WHERE c.id=page.capture_id;
    RETURN NULL;
END $$;
CREATE TRIGGER custody_hash_entries_deleted
    AFTER DELETE ON custody_hash_spool_entries REFERENCING OLD TABLE AS custody_hash_deleted
    FOR EACH STATEMENT EXECUTE FUNCTION custody_hash_spool_account_entries_deleted();

-- Independent provisional v4 per-file chunk checkpoints.  These record only
-- hash metadata and cannot authorize a custody receipt or source deletion.
CREATE TABLE inbound_retained_chunk_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    max_captures bigint NOT NULL DEFAULT 1024 CHECK (max_captures > 0),
    max_bytes bigint NOT NULL DEFAULT 1073741824 CHECK (max_bytes > 0),
    captures bigint NOT NULL DEFAULT 0 CHECK (captures >= 0),
    bytes bigint NOT NULL DEFAULT 0 CHECK (bytes >= 0)
);
INSERT INTO inbound_retained_chunk_budget(singleton) VALUES(true);
CREATE TABLE inbound_retained_chunk_checkpoints (
    work_item_id uuid PRIMARY KEY REFERENCES inbound_retained_sources(work_item_id) ON DELETE RESTRICT,
    capture_id uuid NOT NULL UNIQUE,
    epoch uuid NOT NULL,
    lease_owner text NOT NULL CHECK (octet_length(lease_owner) BETWEEN 1 AND 512),
    lease_token uuid NOT NULL,
    format_version smallint NOT NULL DEFAULT 4 CHECK (format_version=4),
    algorithm text NOT NULL DEFAULT 'blake3-binary-carry-v4' CHECK (algorithm='blake3-binary-carry-v4'),
    chunk_bytes integer NOT NULL DEFAULT 65536 CHECK (chunk_bytes=65536),
    path text NOT NULL CHECK (octet_length(path) BETWEEN 1 AND 262200),
    root_generation text NOT NULL CHECK (octet_length(root_generation) BETWEEN 1 AND 512),
    candidate_generation text NOT NULL CHECK (octet_length(candidate_generation) BETWEEN 1 AND 512),
    file_length bigint NOT NULL CHECK(file_length >= 0),
    next_index bigint NOT NULL DEFAULT 0 CHECK(next_index >= 0),
    revision bigint NOT NULL DEFAULT 0 CHECK(revision >= 0),
    peaks jsonb NOT NULL DEFAULT '[]'::jsonb CHECK(jsonb_typeof(peaks)='array' AND octet_length(peaks::text)<=8192),
    complete boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX inbound_retained_chunk_checkpoints_gc_idx ON inbound_retained_chunk_checkpoints(updated_at,work_item_id);

-- Provisional same-stream v4 baseline evidence, never a receipt by itself.
CREATE TABLE inbound_retained_baseline_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    max_captures bigint NOT NULL DEFAULT 1024 CHECK(max_captures>0),
    max_rows bigint NOT NULL DEFAULT 1000000 CHECK(max_rows>0),
    max_bytes bigint NOT NULL DEFAULT 1073741824 CHECK(max_bytes>0),
    captures bigint NOT NULL DEFAULT 0 CHECK(captures>=0),
    rows bigint NOT NULL DEFAULT 0 CHECK(rows>=0),
    bytes bigint NOT NULL DEFAULT 0 CHECK(bytes>=0)
);
INSERT INTO inbound_retained_baseline_budget(singleton) VALUES(true);
CREATE TABLE inbound_retained_baselines (
    id uuid PRIMARY KEY,
    work_item_id uuid NOT NULL UNIQUE,
    lease_owner text NOT NULL,
    stage_capture_id uuid NOT NULL,
    protected_claim_epoch uuid,
    stage_epoch uuid NOT NULL,
    stage_digest text NOT NULL CHECK(stage_digest ~ '^v3:[0-9a-f]{64}$'),
    stage_generation text NOT NULL CHECK(octet_length(stage_generation) BETWEEN 1 AND 512),
    stage_binding text NOT NULL CHECK(octet_length(stage_binding) BETWEEN 1 AND 262200),
    state text NOT NULL CHECK(state IN ('capturing','frozen','authoritative','discarding')),
    protected_coverage boolean NOT NULL DEFAULT false,
    lifecycle_lock_name text,
    lifecycle_root_generation text CHECK(lifecycle_root_generation IS NULL OR octet_length(lifecycle_root_generation) BETWEEN 1 AND 512),
    lifecycle_lock_generation text CHECK(lifecycle_lock_generation IS NULL OR octet_length(lifecycle_lock_generation) BETWEEN 1 AND 512),
    cleaned_attempt_id uuid,
    cleaned_attempt_at timestamptz,
    cleaned_session_id uuid,
    cleaned_root_generation text CHECK(cleaned_root_generation IS NULL OR octet_length(cleaned_root_generation) BETWEEN 1 AND 512),
    cleaned_final_parent_generation text CHECK(cleaned_final_parent_generation IS NULL OR octet_length(cleaned_final_parent_generation) BETWEEN 1 AND 512),
    file_count bigint NOT NULL DEFAULT 0 CHECK(file_count>=0),
    chunk_count bigint NOT NULL DEFAULT 0 CHECK(chunk_count>=0),
    logical_bytes bigint NOT NULL CHECK(logical_bytes>0),
    cleanup_after timestamptz NOT NULL DEFAULT clock_timestamp(),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (((state<>'authoritative' AND lifecycle_lock_name IS NULL AND lifecycle_root_generation IS NULL AND lifecycle_lock_generation IS NULL) OR
           (state='authoritative' AND lifecycle_lock_name=('stage-' || replace(id::text,'-','') || '.lock') AND lifecycle_root_generation IS NOT NULL AND lifecycle_lock_generation IS NOT NULL)) IS TRUE),
    CHECK (((cleaned_attempt_id IS NULL AND cleaned_attempt_at IS NULL AND cleaned_session_id IS NULL AND cleaned_root_generation IS NULL AND cleaned_final_parent_generation IS NULL) OR
           (state='authoritative' AND cleaned_attempt_id IS NOT NULL AND cleaned_attempt_at IS NOT NULL AND cleaned_session_id IS NOT NULL AND cleaned_root_generation IS NOT NULL AND cleaned_final_parent_generation IS NOT NULL)) IS TRUE)
);
CREATE INDEX inbound_retained_baseline_gc_idx ON inbound_retained_baselines(cleanup_after,id) WHERE state<>'authoritative';
CREATE TABLE inbound_retained_baseline_files (
    baseline_id uuid NOT NULL REFERENCES inbound_retained_baselines(id) ON DELETE RESTRICT,
    stage_ordinal bigint NOT NULL,
    path_key text NOT NULL CHECK(octet_length(path_key) BETWEEN 1 AND 262200),
    parent_key text NOT NULL CHECK(octet_length(parent_key)<=262200),
    file_length bigint NOT NULL CHECK(file_length>=0),
    stage_digest text NOT NULL CHECK(stage_digest ~ '^[0-9a-f]{64}$'),
    -- Metadata only: kernel fs-verity digest and exact Linux inode identity.
    -- The file row's fixed 256-byte accounting allowance covers these fields.
    stage_verity_sha256 text CHECK(stage_verity_sha256 IS NULL OR stage_verity_sha256 ~ '^[0-9a-f]{64}$'),
    stage_verity_device text CHECK(stage_verity_device IS NULL OR stage_verity_device ~ '^[0-9a-f]{16}$'),
    stage_verity_inode text CHECK(stage_verity_inode IS NULL OR stage_verity_inode ~ '^[0-9a-f]{16}$'),
    chunk_count bigint NOT NULL DEFAULT 0 CHECK(chunk_count>=0),
    finished boolean NOT NULL DEFAULT false,
    logical_bytes bigint NOT NULL CHECK(logical_bytes>0),
    PRIMARY KEY(baseline_id,stage_ordinal),
    CHECK ((stage_verity_sha256 IS NULL AND stage_verity_device IS NULL AND stage_verity_inode IS NULL) OR
           (stage_verity_sha256 IS NOT NULL AND stage_verity_device IS NOT NULL AND stage_verity_inode IS NOT NULL))
);
CREATE INDEX inbound_retained_baseline_path_idx ON inbound_retained_baseline_files(baseline_id,ingest_occurrence_hash(path_key));
-- Directory rows include synthetic root (stage_ordinal=0). Mutable scan
-- hints are not filesystem truth and cannot prove absence or authorize deletion.
CREATE TABLE inbound_retained_baseline_directories (
    baseline_id uuid NOT NULL REFERENCES inbound_retained_baselines(id) ON DELETE RESTRICT,
    stage_ordinal bigint NOT NULL CHECK(stage_ordinal>=0),
    path_key text NOT NULL CHECK(octet_length(path_key)<=262200),
    parent_key text NOT NULL CHECK(octet_length(parent_key)<=262200),
    stage_digest text NOT NULL CHECK(stage_digest ~ '^[0-9a-f]{64}$'),
    logical_bytes bigint NOT NULL CHECK(logical_bytes>0),
    scan_cookie text CHECK(scan_cookie IS NULL OR (octet_length(scan_cookie)=16 AND translate(scan_cookie,'0123456789abcdef','')='')),
    scan_generation text CHECK(scan_generation IS NULL OR octet_length(scan_generation) BETWEEN 1 AND 512),
    scan_revision bigint NOT NULL DEFAULT 0 CHECK(scan_revision>=0),
    next_scan_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(baseline_id,stage_ordinal),
    CHECK ((stage_ordinal=0 AND path_key='' AND parent_key='') OR (stage_ordinal>0 AND path_key<>''))
);
CREATE INDEX inbound_retained_baseline_dir_path_idx ON inbound_retained_baseline_directories(baseline_id,ingest_occurrence_hash(path_key));
CREATE INDEX inbound_retained_baseline_dir_due_idx ON inbound_retained_baseline_directories(baseline_id,next_scan_at,stage_ordinal);
CREATE TABLE inbound_retained_baseline_chunks (
    baseline_id uuid NOT NULL,
    stage_ordinal bigint NOT NULL,
    chunk_index bigint NOT NULL CHECK(chunk_index>=0),
    chunk_length integer NOT NULL CHECK(chunk_length BETWEEN 1 AND 65536),
    digest text NOT NULL CHECK(digest ~ '^[0-9a-f]{64}$'),
    PRIMARY KEY(baseline_id,stage_ordinal,chunk_index),
    FOREIGN KEY(baseline_id,stage_ordinal) REFERENCES inbound_retained_baseline_files(baseline_id,stage_ordinal) ON DELETE RESTRICT
);
CREATE FUNCTION inbound_retained_baseline_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.protected_coverage AND NOT NEW.protected_coverage THEN
        RAISE EXCEPTION 'protected baseline coverage cannot be revoked';
    END IF;
    IF NEW.protected_coverage AND NOT OLD.protected_coverage AND
       (NEW.state NOT IN ('frozen','authoritative') OR EXISTS
        (SELECT 1 FROM inbound_retained_baseline_files f WHERE f.baseline_id=NEW.id AND
         (NOT f.finished OR f.stage_verity_sha256 IS NULL OR f.stage_verity_device IS NULL OR f.stage_verity_inode IS NULL))) THEN
        RAISE EXCEPTION 'protected baseline file evidence is incomplete';
    END IF;
    IF NEW.state='authoritative' AND OLD.state<>'authoritative' AND
       (NOT NEW.protected_coverage OR NEW.lifecycle_lock_name IS NULL OR NEW.lifecycle_root_generation IS NULL OR NEW.lifecycle_lock_generation IS NULL OR NOT EXISTS
        (SELECT 1 FROM release_work_item_staging_handoffs h WHERE h.work_item_id=NEW.work_item_id AND h.status='staged' AND h.retained_source)) THEN
        RAISE EXCEPTION 'authoritative baseline requires protected coverage and retained staging receipt';
    END IF;
    IF NEW.id<>OLD.id OR NEW.work_item_id<>OLD.work_item_id OR NEW.lease_owner<>OLD.lease_owner OR
       NEW.stage_capture_id<>OLD.stage_capture_id OR NEW.protected_claim_epoch IS DISTINCT FROM OLD.protected_claim_epoch OR NEW.stage_epoch<>OLD.stage_epoch OR
       NEW.stage_digest<>OLD.stage_digest OR NEW.stage_generation<>OLD.stage_generation OR
       NEW.stage_binding COLLATE "C"<>OLD.stage_binding COLLATE "C" OR NEW.logical_bytes<>OLD.logical_bytes OR
       NOT ((OLD.state='capturing' AND NEW.state IN ('capturing','frozen','discarding')) OR
            (OLD.state='frozen' AND NEW.state IN ('frozen','authoritative','discarding')) OR
            (OLD.state='authoritative' AND (NEW IS NOT DISTINCT FROM OLD OR
              (OLD.cleaned_attempt_id IS NULL AND OLD.cleaned_attempt_at IS NULL AND NEW.cleaned_attempt_id IS NOT NULL AND NEW.cleaned_attempt_at IS NOT NULL AND NEW.cleaned_session_id IS NOT NULL AND NEW.cleaned_root_generation IS NOT NULL AND NEW.cleaned_final_parent_generation IS NOT NULL AND
               (to_jsonb(NEW)-'cleaned_attempt_id'-'cleaned_attempt_at'-'cleaned_session_id'-'cleaned_root_generation'-'cleaned_final_parent_generation')=(to_jsonb(OLD)-'cleaned_attempt_id'-'cleaned_attempt_at'-'cleaned_session_id'-'cleaned_root_generation'-'cleaned_final_parent_generation')))) OR
            (OLD.state='discarding' AND NEW.state='discarding')) OR
       (OLD.state='frozen' AND (NEW.file_count<>OLD.file_count OR NEW.chunk_count<>OLD.chunk_count)) THEN
        RAISE EXCEPTION 'retained baseline evidence is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER inbound_retained_baseline_guard BEFORE UPDATE ON inbound_retained_baselines FOR EACH ROW EXECUTE FUNCTION inbound_retained_baseline_guard();
CREATE FUNCTION inbound_retained_baseline_child_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE s text; b uuid;
BEGIN
    IF TG_OP='DELETE' THEN b:=OLD.baseline_id; ELSE b:=NEW.baseline_id; END IF;
    SELECT state INTO s FROM inbound_retained_baselines WHERE id=b;
    IF (TG_OP='DELETE' AND s<>'discarding') OR (TG_OP<>'DELETE' AND s<>'capturing') THEN
       RAISE EXCEPTION 'frozen retained baseline evidence is immutable';
    END IF;
    IF TG_OP='UPDATE' AND (NEW.baseline_id<>OLD.baseline_id OR NEW.stage_ordinal<>OLD.stage_ordinal) THEN
       RAISE EXCEPTION 'retained baseline identity is immutable';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;
CREATE TRIGGER inbound_retained_baseline_file_guard BEFORE INSERT OR UPDATE OR DELETE ON inbound_retained_baseline_files FOR EACH ROW EXECUTE FUNCTION inbound_retained_baseline_child_guard();
CREATE TRIGGER inbound_retained_baseline_chunk_guard BEFORE INSERT OR UPDATE OR DELETE ON inbound_retained_baseline_chunks FOR EACH ROW EXECUTE FUNCTION inbound_retained_baseline_child_guard();
CREATE FUNCTION inbound_retained_baseline_directory_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE s text; b uuid;
BEGIN
    IF TG_OP='DELETE' THEN b:=OLD.baseline_id; ELSE b:=NEW.baseline_id; END IF;
    SELECT state INTO s FROM inbound_retained_baselines WHERE id=b;
    IF TG_OP='INSERT' AND s<>'capturing' OR TG_OP='DELETE' AND s<>'discarding' THEN
        RAISE EXCEPTION 'directory evidence lifecycle changed';
    END IF;
    IF TG_OP='UPDATE' AND (s<>'authoritative' OR NEW.baseline_id<>OLD.baseline_id OR NEW.stage_ordinal<>OLD.stage_ordinal OR
       NEW.path_key COLLATE "C"<>OLD.path_key COLLATE "C" OR NEW.parent_key COLLATE "C"<>OLD.parent_key COLLATE "C" OR
       NEW.stage_digest<>OLD.stage_digest OR NEW.logical_bytes<>OLD.logical_bytes) THEN
        RAISE EXCEPTION 'directory evidence is immutable';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;
CREATE TRIGGER inbound_retained_baseline_directory_guard BEFORE INSERT OR UPDATE OR DELETE ON inbound_retained_baseline_directories FOR EACH ROW EXECUTE FUNCTION inbound_retained_baseline_directory_guard();

-- A purge owns its exact occurrence until its filesystem operation and DB
-- cleanup are durably reconciled.  A crashed intent is deliberately not
-- expired automatically: doing so could expose retained source bytes.
-- Metadata only. Reserved identities do not authorize reopening or deleting
-- any filesystem object; ready identities require an externally quiesced,
-- maintenance-fenced private capability and cannot be rewritten or removed.
CREATE TABLE work_item_lifecycle_maintenance (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    owner text, token uuid, lease_expires_at timestamptz, quiesced boolean NOT NULL DEFAULT false,
    recovery_reservation_id uuid, recovery_reservation_token uuid,
    cursor_id uuid, pass_complete boolean NOT NULL DEFAULT false, enforcement_enabled boolean NOT NULL DEFAULT false,
    CHECK ((owner IS NULL AND token IS NULL AND lease_expires_at IS NULL AND NOT quiesced AND recovery_reservation_id IS NULL AND recovery_reservation_token IS NULL) OR
           (owner IS NOT NULL AND token IS NOT NULL AND lease_expires_at IS NOT NULL)),
    CHECK ((recovery_reservation_id IS NULL AND recovery_reservation_token IS NULL) OR
           (recovery_reservation_id IS NOT NULL AND recovery_reservation_token IS NOT NULL AND NOT quiesced))
);
INSERT INTO work_item_lifecycle_maintenance(singleton) VALUES(true);
CREATE TABLE work_item_lifecycle_attestation_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    entries bigint NOT NULL DEFAULT 0 CHECK(entries>=0), max_entries bigint NOT NULL DEFAULT 10000 CHECK(max_entries>0)
);
INSERT INTO work_item_lifecycle_attestation_budget(singleton) VALUES(true);
CREATE TABLE work_item_lifecycle_attestations (
    token uuid PRIMARY KEY, actor text NOT NULL CHECK(octet_length(actor) BETWEEN 1 AND 512),
    reason text NOT NULL CHECK(octet_length(reason) BETWEEN 1 AND 512),
    backup_reference text NOT NULL CHECK(octet_length(backup_reference) BETWEEN 1 AND 512),
    recovery_reservation_id uuid,
    recovery_reservation_token uuid,
    stop_all_writers boolean NOT NULL CHECK(stop_all_writers),
    inspection_complete boolean NOT NULL CHECK(inspection_complete),
    attested_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK ((recovery_reservation_id IS NULL) = (recovery_reservation_token IS NULL))
);
CREATE FUNCTION preserve_lifecycle_attestation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'lifecycle maintenance attestations are append-only'; END $$;
CREATE TRIGGER preserve_lifecycle_attestation BEFORE UPDATE OR DELETE ON work_item_lifecycle_attestations
    FOR EACH ROW EXECUTE FUNCTION preserve_lifecycle_attestation();
CREATE TABLE work_item_lifecycle_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    entries bigint NOT NULL DEFAULT 0 CHECK(entries>=0), max_entries bigint NOT NULL DEFAULT 10000000 CHECK(max_entries>0)
);
INSERT INTO work_item_lifecycle_budget(singleton) VALUES(true);
CREATE TABLE work_item_lifecycle_locks (
    work_item_id uuid PRIMARY KEY, source_library_id uuid, source_path text NOT NULL CHECK(octet_length(source_path) BETWEEN 1 AND 262200),
    source_path_format smallint NOT NULL CHECK(source_path_format IN (0,1)),
    lock_name text NOT NULL UNIQUE CHECK(lock_name=('work-' || replace(work_item_id::text,'-','') || '.lock')),
    state text NOT NULL CHECK(state IN ('reserved','ready')),
    lock_root_generation text CHECK(lock_root_generation IS NULL OR octet_length(lock_root_generation) BETWEEN 1 AND 512),
    lock_generation text CHECK(lock_generation IS NULL OR octet_length(lock_generation) BETWEEN 1 AND 512),
    stage_root_generation text CHECK(stage_root_generation IS NULL OR octet_length(stage_root_generation) BETWEEN 1 AND 512),
    completed_token uuid, completed_owner text,
    CHECK ((completed_token IS NULL) = (completed_owner IS NULL)),
    CHECK (state='ready' OR completed_token IS NULL),
    CHECK ((state='reserved' AND lock_root_generation IS NULL AND lock_generation IS NULL AND stage_root_generation IS NULL) OR
           (state='ready' AND lock_root_generation IS NOT NULL AND lock_generation IS NOT NULL AND stage_root_generation IS NOT NULL))
);
CREATE TABLE work_item_lifecycle_provision_jobs (
    work_item_id uuid PRIMARY KEY REFERENCES work_item_lifecycle_locks(work_item_id) ON DELETE RESTRICT,
    owner text, token uuid, lease_expires_at timestamptz,
    due_at timestamptz NOT NULL DEFAULT clock_timestamp(), retry_count integer NOT NULL DEFAULT 0 CHECK(retry_count>=0),
    last_reason text CHECK(last_reason IS NULL OR last_reason IN ('ambiguous','missing','changed','unreadable','unsupported')),
    CHECK ((owner IS NULL AND token IS NULL AND lease_expires_at IS NULL) OR
           (owner IS NOT NULL AND token IS NOT NULL AND lease_expires_at IS NOT NULL))
);
CREATE INDEX work_item_lifecycle_provision_due_idx ON work_item_lifecycle_provision_jobs(due_at,work_item_id);
CREATE FUNCTION enroll_new_work_item_lifecycle() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (SELECT enforcement_enabled FROM work_item_lifecycle_maintenance WHERE singleton) THEN
        UPDATE work_item_lifecycle_budget SET entries=entries+1 WHERE singleton AND entries<max_entries;
        IF NOT FOUND THEN RAISE EXCEPTION 'lifecycle identity capacity exhausted' USING ERRCODE='53400'; END IF;
        INSERT INTO work_item_lifecycle_locks(work_item_id,source_library_id,source_path,source_path_format,lock_name,state)
            VALUES(NEW.id,NEW.source_library_id,NEW.source_path,NEW.discovery_path_format,
                   'work-' || replace(NEW.id::text,'-','') || '.lock','reserved');
        INSERT INTO work_item_lifecycle_provision_jobs(work_item_id) VALUES(NEW.id);
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER enroll_new_work_item_lifecycle AFTER INSERT ON release_work_items
    FOR EACH ROW EXECUTE FUNCTION enroll_new_work_item_lifecycle();
CREATE TABLE work_item_lifecycle_quarantine (
    work_item_id uuid PRIMARY KEY,
    reason text NOT NULL CHECK(reason IN ('ambiguous','missing','changed','unreadable','unsupported')),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX work_item_lifecycle_reserved_idx ON work_item_lifecycle_locks(work_item_id) WHERE state='reserved';
CREATE FUNCTION preserve_work_item_lifecycle_lock() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN RAISE EXCEPTION 'lifecycle lock identity cannot be deleted'; END IF;
    IF OLD.state='ready' OR NEW.work_item_id<>OLD.work_item_id OR
       NEW.source_library_id IS DISTINCT FROM OLD.source_library_id OR NEW.source_path COLLATE "C"<>OLD.source_path COLLATE "C" OR
       NEW.source_path_format<>OLD.source_path_format OR NEW.lock_name<>OLD.lock_name OR NEW.state<>'ready' THEN
        RAISE EXCEPTION 'lifecycle lock identity is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_work_item_lifecycle_lock BEFORE UPDATE OR DELETE ON work_item_lifecycle_locks
    FOR EACH ROW EXECUTE FUNCTION preserve_work_item_lifecycle_lock();

-- In-progress administrative media operations pin exact item/release scopes.
-- Intents have no automatic timeout: loss of a process cannot authorize a
-- competing writer while its filesystem descriptors might still be live.
CREATE TABLE admin_media_reservation_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    reservations bigint NOT NULL DEFAULT 0 CHECK(reservations>=0),
    max_reservations bigint NOT NULL DEFAULT 1024 CHECK(max_reservations>0)
);
INSERT INTO admin_media_reservation_budget(singleton) VALUES(true);
CREATE TABLE admin_media_reservations (
    id uuid PRIMARY KEY, owner text NOT NULL CHECK(octet_length(owner) BETWEEN 1 AND 512),
    token uuid NOT NULL UNIQUE, lease_expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE admin_media_reservation_items (
    work_item_id uuid PRIMARY KEY, reservation_id uuid NOT NULL REFERENCES admin_media_reservations(id) ON DELETE RESTRICT,
    source_library_id uuid, source_path text NOT NULL CHECK(octet_length(source_path) BETWEEN 1 AND 262200)
);
CREATE INDEX admin_media_reservation_occurrence_idx ON admin_media_reservation_items(source_library_id,ingest_occurrence_hash(source_path));
CREATE INDEX admin_media_reservation_items_scope_idx ON admin_media_reservation_items(reservation_id,work_item_id);
CREATE TABLE admin_media_reservation_releases (
    release_id uuid PRIMARY KEY, reservation_id uuid NOT NULL REFERENCES admin_media_reservations(id) ON DELETE RESTRICT
);
CREATE INDEX admin_media_reservation_releases_scope_idx ON admin_media_reservation_releases(reservation_id,release_id);

-- These are provisional expectations, not proof that a root was opened or
-- topology was checked. The filesystem executor must independently verify
-- every root under the external maintenance fence before use.
CREATE TABLE admin_media_root_sets (
    revision uuid PRIMARY KEY,
    reservation_id uuid NOT NULL UNIQUE REFERENCES admin_media_reservations(id) ON DELETE RESTRICT,
    maintenance_token uuid NOT NULL,
    reservation_token uuid NOT NULL,
    entry_count integer NOT NULL CHECK(entry_count BETWEEN 5 AND 132),
    logical_bytes bigint NOT NULL CHECK(logical_bytes BETWEEN 1 AND 131072)
);
CREATE TABLE admin_media_root_entries (
    revision uuid NOT NULL REFERENCES admin_media_root_sets(revision) ON DELETE RESTRICT,
    ordinal smallint NOT NULL CHECK(ordinal BETWEEN 1 AND 132),
    role text NOT NULL CHECK(role IN ('source','work','stage','canonical','asset','lock')),
    source_library_id uuid,
    path_format text NOT NULL CHECK(path_format IN ('unix-hex-v1','windows-u16-hex-v1')),
    path_hex text NOT NULL CHECK(length(path_hex) BETWEEN 2 AND 131072 AND path_hex ~ '^[0-9a-f]+$'),
    generation text NOT NULL CHECK(octet_length(generation) BETWEEN 1 AND 256),
    mount_id text NOT NULL CHECK(mount_id ~ '^[0-9a-f]{16}$'),
    filesystem_magic text NOT NULL CHECK(filesystem_magic ~ '^[0-9a-f]{16}$'),
    PRIMARY KEY(revision,ordinal),
    UNIQUE NULLS NOT DISTINCT(revision,role,source_library_id),
    CHECK(role='source' OR source_library_id IS NULL)
);
CREATE FUNCTION preserve_admin_media_root_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- A snapshot can be removed only before any intent or acknowledgement.
    -- Its detached provisional root metadata can then be removed in pages.
    IF TG_OP='DELETE' AND NOT EXISTS(SELECT 1 FROM admin_media_snapshots WHERE root_set_revision=OLD.revision) THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'bound provisional root metadata is immutable';
END $$;
CREATE TRIGGER preserve_admin_media_root_set BEFORE UPDATE OR DELETE ON admin_media_root_sets
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_root_binding();
CREATE TRIGGER preserve_admin_media_root_entry BEFORE UPDATE OR DELETE ON admin_media_root_entries
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_root_binding();
CREATE FUNCTION bound_admin_media_root_entry_insert() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE expected integer;
BEGIN
    SELECT entry_count INTO expected FROM admin_media_root_sets WHERE revision=NEW.revision FOR UPDATE;
    IF expected IS NULL OR NEW.ordinal>expected OR
       (SELECT count(*) FROM admin_media_root_entries WHERE revision=NEW.revision)>=expected OR
       EXISTS(SELECT 1 FROM admin_media_snapshots WHERE root_set_revision=NEW.revision) THEN
        RAISE EXCEPTION 'root set is complete or already bound';
    END IF;
    RETURN NEW;
END $$;

-- Exact bounded operator target selection is bound before any filesystem page.
CREATE TABLE admin_media_delete_scopes (
    reservation_id uuid PRIMARY KEY REFERENCES admin_media_reservations(id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK(kind IN ('release','track','artist'))
);
CREATE TABLE admin_media_delete_targets (
    reservation_id uuid NOT NULL REFERENCES admin_media_delete_scopes(reservation_id) ON DELETE RESTRICT,
    internal_id uuid NOT NULL, public_id uuid NOT NULL, parent_release_id uuid,
    PRIMARY KEY(reservation_id,internal_id), UNIQUE(reservation_id,public_id)
);
-- Offline media deletion work is captured and acknowledged in bounded pages.
-- Paths and cryptographic metadata are not media payloads.
CREATE TABLE admin_media_snapshot_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton), snapshots bigint NOT NULL DEFAULT 0 CHECK(snapshots>=0),
    entries bigint NOT NULL DEFAULT 0 CHECK(entries>=0), bytes bigint NOT NULL DEFAULT 0 CHECK(bytes>=0),
    max_snapshots bigint NOT NULL DEFAULT 1024, max_entries bigint NOT NULL DEFAULT 2000000, max_bytes bigint NOT NULL DEFAULT 536870912
);
INSERT INTO admin_media_snapshot_budget(singleton) VALUES(true);
CREATE TABLE admin_media_snapshots (
    reservation_id uuid PRIMARY KEY REFERENCES admin_media_reservations(id) ON DELETE RESTRICT,
    revision uuid NOT NULL DEFAULT gen_random_uuid(),
    root_set_revision uuid REFERENCES admin_media_root_sets(revision) ON DELETE RESTRICT,
    state text NOT NULL CHECK(state IN ('capturing','frozen','discarding')),
    capture_phase text NOT NULL DEFAULT 'media' CHECK(capture_phase IN ('media','directories','entity_images','provider_images','artist_images','artist_provider_images','complete')),
    aux_release uuid, aux_id uuid,
    cursor_release uuid, cursor_ordinal integer, cursor_track uuid, cursor_asset uuid, cursor_purpose text,
    entries bigint NOT NULL DEFAULT 0 CHECK(entries>=0), bytes bigint NOT NULL DEFAULT 0 CHECK(bytes>=0),
    CHECK ((cursor_release IS NULL AND cursor_ordinal IS NULL AND cursor_track IS NULL AND cursor_asset IS NULL AND cursor_purpose IS NULL) OR
           (cursor_release IS NOT NULL AND cursor_ordinal IS NOT NULL AND cursor_track IS NOT NULL AND cursor_asset IS NOT NULL AND cursor_purpose IS NOT NULL))
);
CREATE TRIGGER bound_admin_media_root_entry_insert BEFORE INSERT ON admin_media_root_entries
    FOR EACH ROW EXECUTE FUNCTION bound_admin_media_root_entry_insert();
CREATE TABLE admin_media_snapshot_entries (
    reservation_id uuid NOT NULL REFERENCES admin_media_snapshots(reservation_id) ON DELETE RESTRICT,
    asset_id uuid NOT NULL, relative_path text NOT NULL CHECK(octet_length(relative_path) BETWEEN 1 AND 262200),
    byte_hash text NOT NULL CHECK(byte_hash ~ '^[0-9a-f]{64}$'), byte_size bigint NOT NULL CHECK(byte_size>=0),
    acknowledgement uuid, PRIMARY KEY(reservation_id,asset_id)
);
CREATE INDEX admin_media_snapshot_pending_idx ON admin_media_snapshot_entries(reservation_id,asset_id) WHERE acknowledgement IS NULL;
CREATE INDEX admin_media_snapshot_ack_idx ON admin_media_snapshot_entries(reservation_id,acknowledgement,asset_id) WHERE acknowledgement IS NOT NULL;

CREATE TABLE admin_media_snapshot_objects (
    reservation_id uuid NOT NULL REFERENCES admin_media_snapshots(reservation_id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK(kind IN ('staging_directory','canonical_directory','artwork')),
    object_id uuid NOT NULL, object_key text NOT NULL CHECK(octet_length(object_key) BETWEEN 1 AND 262200),
    byte_hash text CHECK(byte_hash IS NULL OR byte_hash ~ '^[0-9a-f]{64}$'), byte_size bigint CHECK(byte_size IS NULL OR byte_size>=0),
    acknowledgement uuid, PRIMARY KEY(reservation_id,kind,object_id),
    CHECK ((kind='artwork' AND byte_hash IS NOT NULL AND byte_size IS NOT NULL AND object_key ~ '^[0-9a-f]{64}-[1-9][0-9]{0,7}$') OR
           (kind<>'artwork' AND byte_hash IS NULL AND byte_size IS NULL))
);
CREATE UNIQUE INDEX admin_media_snapshot_artwork_unique ON admin_media_snapshot_objects(reservation_id,object_key) WHERE kind='artwork';
CREATE INDEX admin_media_snapshot_objects_pending ON admin_media_snapshot_objects(reservation_id,kind,object_id) WHERE acknowledgement IS NULL;
CREATE INDEX admin_media_snapshot_objects_ack ON admin_media_snapshot_objects(reservation_id,acknowledgement,kind,object_id) WHERE acknowledgement IS NOT NULL;
-- Offline closure reference checks need indexed exact asset-key lookups.
CREATE INDEX admin_closure_catalog_entity_asset_idx ON catalog_entity_images(asset_key);
CREATE INDEX admin_closure_catalog_provider_asset_idx ON catalog_provider_images(asset_key);
CREATE INDEX admin_closure_catalog_provider_release_idx ON catalog_provider_images(entity_type,entity_id,id);
CREATE INDEX admin_closure_native_avatar_asset_idx ON native_user_avatars(asset_key);
CREATE INDEX admin_closure_native_theme_asset_idx ON native_theme_assets(asset_key);
CREATE INDEX admin_closure_native_playlist_asset_idx ON native_playlists(asset_key) WHERE asset_key IS NOT NULL;
CREATE INDEX admin_closure_native_users_asset_idx ON native_users(asset_key) WHERE asset_key IS NOT NULL;

CREATE FUNCTION preserve_admin_media_snapshot() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.state IN ('capturing','frozen') AND NEW.state='discarding' AND
       (to_jsonb(NEW)-'state')=(to_jsonb(OLD)-'state') AND
       NOT EXISTS(SELECT 1 FROM admin_media_page_intents WHERE reservation_id=OLD.reservation_id) AND
       NOT EXISTS(SELECT 1 FROM admin_media_snapshot_entries WHERE reservation_id=OLD.reservation_id AND acknowledgement IS NOT NULL) AND
       NOT EXISTS(SELECT 1 FROM admin_media_snapshot_objects WHERE reservation_id=OLD.reservation_id AND acknowledgement IS NOT NULL) THEN
        RETURN NEW;
    END IF;
    IF NEW.reservation_id<>OLD.reservation_id OR NEW.revision<>OLD.revision OR
       (OLD.root_set_revision IS NOT NULL AND NEW.root_set_revision IS DISTINCT FROM OLD.root_set_revision) OR
       (OLD.state='frozen' AND NEW IS DISTINCT FROM OLD) OR
       NOT ((OLD.state='capturing' AND NEW.state IN ('capturing','frozen')) OR (OLD.state='frozen' AND NEW.state='frozen') OR (OLD.state='discarding' AND NEW IS NOT DISTINCT FROM OLD)) THEN
        RAISE EXCEPTION 'media snapshot revision and frozen state are immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_admin_media_snapshot BEFORE UPDATE ON admin_media_snapshots
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_snapshot();

CREATE FUNCTION preserve_admin_media_snapshot_entry() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF NOT (OLD.acknowledgement IS NULL AND EXISTS(SELECT 1 FROM admin_media_snapshots WHERE reservation_id=OLD.reservation_id AND state='discarding')) AND
           NOT EXISTS(SELECT 1 FROM admin_media_completion_receipts WHERE reservation_id=OLD.reservation_id AND cleanup_state='cleaning') THEN
            RAISE EXCEPTION 'media evidence requires fenced discard or explicit repair';
        END IF;
        RETURN OLD;
    END IF;
    IF NEW.reservation_id<>OLD.reservation_id OR NEW.asset_id<>OLD.asset_id OR
       NEW.relative_path COLLATE "C"<>OLD.relative_path COLLATE "C" OR NEW.byte_hash<>OLD.byte_hash OR NEW.byte_size<>OLD.byte_size OR
       (OLD.acknowledgement IS NOT NULL AND NEW.acknowledgement IS DISTINCT FROM OLD.acknowledgement) THEN
        RAISE EXCEPTION 'media snapshot evidence is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_admin_media_snapshot_entry BEFORE UPDATE OR DELETE ON admin_media_snapshot_entries
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_snapshot_entry();
CREATE FUNCTION preserve_admin_media_snapshot_object() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF NOT (OLD.acknowledgement IS NULL AND EXISTS(SELECT 1 FROM admin_media_snapshots WHERE reservation_id=OLD.reservation_id AND state='discarding')) AND
           NOT EXISTS(SELECT 1 FROM admin_media_completion_receipts WHERE reservation_id=OLD.reservation_id AND cleanup_state='cleaning') THEN
            RAISE EXCEPTION 'object evidence requires fenced discard or explicit repair';
        END IF;
        RETURN OLD;
    END IF;
    IF NEW.reservation_id<>OLD.reservation_id OR NEW.kind<>OLD.kind OR NEW.object_id<>OLD.object_id OR
       NEW.object_key COLLATE "C"<>OLD.object_key COLLATE "C" OR NEW.byte_hash IS DISTINCT FROM OLD.byte_hash OR NEW.byte_size IS DISTINCT FROM OLD.byte_size OR
       (OLD.acknowledgement IS NOT NULL AND NEW.acknowledgement IS DISTINCT FROM OLD.acknowledgement) THEN
        RAISE EXCEPTION 'object snapshot evidence is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_admin_media_snapshot_object BEFORE UPDATE OR DELETE ON admin_media_snapshot_objects
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_snapshot_object();

-- A page intent records exactly what was planned before any filesystem work.
-- It is not evidence that the operation happened; interrupted intents stay
-- durable and cannot be adopted by a different lease epoch without revalidation.
CREATE TABLE admin_media_page_intent_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton), intents bigint NOT NULL DEFAULT 0 CHECK(intents>=0),
    entries bigint NOT NULL DEFAULT 0 CHECK(entries>=0), bytes bigint NOT NULL DEFAULT 0 CHECK(bytes>=0),
    max_intents bigint NOT NULL DEFAULT 125000, max_entries bigint NOT NULL DEFAULT 2000000, max_bytes bigint NOT NULL DEFAULT 536870912
);
INSERT INTO admin_media_page_intent_budget(singleton) VALUES(true);
CREATE TABLE admin_media_page_intents (
    id uuid PRIMARY KEY, reservation_id uuid NOT NULL REFERENCES admin_media_snapshots(reservation_id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK(kind IN ('media','object')), maintenance_token uuid NOT NULL, reservation_token uuid NOT NULL,
    snapshot_revision uuid NOT NULL,
    root_set_revision uuid REFERENCES admin_media_root_sets(revision) ON DELETE RESTRICT,
    state text NOT NULL CHECK(state IN ('intent','verified','uncertain')),
    entry_count integer NOT NULL CHECK(entry_count BETWEEN 1 AND 16), logical_bytes bigint NOT NULL CHECK(logical_bytes>0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE UNIQUE INDEX admin_media_page_one_open_intent ON admin_media_page_intents(reservation_id) WHERE state IN ('intent','uncertain');
CREATE INDEX admin_media_page_intent_reservation_idx ON admin_media_page_intents(reservation_id,id);
CREATE TABLE admin_media_page_intent_entries (
    intent_id uuid NOT NULL REFERENCES admin_media_page_intents(id) ON DELETE RESTRICT,
    ordinal smallint NOT NULL CHECK(ordinal BETWEEN 1 AND 16), kind text NOT NULL,
    object_id uuid NOT NULL, object_key text NOT NULL CHECK(octet_length(object_key) BETWEEN 1 AND 262200),
    byte_hash text CHECK(byte_hash IS NULL OR byte_hash ~ '^[0-9a-f]{64}$'), byte_size bigint CHECK(byte_size IS NULL OR byte_size>=0),
    PRIMARY KEY(intent_id,ordinal), UNIQUE(intent_id,kind,object_id)
);
CREATE FUNCTION preserve_admin_media_page_intent() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF EXISTS(SELECT 1 FROM admin_media_completion_receipts WHERE reservation_id=OLD.reservation_id AND cleanup_state='cleaning') THEN RETURN OLD; END IF;
        RAISE EXCEPTION 'page intent requires explicit fenced cleanup';
    END IF;
    IF NEW.id<>OLD.id OR NEW.reservation_id<>OLD.reservation_id OR NEW.kind<>OLD.kind OR
       NEW.maintenance_token<>OLD.maintenance_token OR NEW.reservation_token<>OLD.reservation_token OR NEW.snapshot_revision<>OLD.snapshot_revision OR
       NEW.root_set_revision IS DISTINCT FROM OLD.root_set_revision OR
       NEW.entry_count<>OLD.entry_count OR NEW.logical_bytes<>OLD.logical_bytes OR
       NOT ((OLD.state='intent' AND NEW.state IN ('intent','verified','uncertain')) OR
            (OLD.state='uncertain' AND NEW.state='uncertain') OR (OLD.state='verified' AND NEW.state='verified')) THEN
        RAISE EXCEPTION 'page intent binding is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_admin_media_page_intent BEFORE UPDATE OR DELETE ON admin_media_page_intents
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_page_intent();
CREATE FUNCTION preserve_admin_media_page_intent_entry() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' AND EXISTS(SELECT 1 FROM admin_media_page_intents i JOIN admin_media_completion_receipts r ON r.reservation_id=i.reservation_id WHERE i.id=OLD.intent_id AND r.cleanup_state='cleaning') THEN RETURN OLD; END IF;
    RAISE EXCEPTION 'page intent entries are immutable';
END $$;
CREATE TRIGGER preserve_admin_media_page_intent_entry BEFORE UPDATE OR DELETE ON admin_media_page_intent_entries
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_page_intent_entry();
CREATE FUNCTION bound_admin_media_page_intent_insert() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE expected integer; current_state text;
BEGIN
    SELECT entry_count,state INTO expected,current_state FROM admin_media_page_intents WHERE id=NEW.intent_id FOR UPDATE;
    IF expected IS NULL OR current_state<>'intent' OR NEW.ordinal>expected OR
       (SELECT count(*) FROM admin_media_page_intent_entries WHERE intent_id=NEW.intent_id)>=expected THEN
        RAISE EXCEPTION 'page intent entry set is complete or closed';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER bound_admin_media_page_intent_insert BEFORE INSERT ON admin_media_page_intent_entries
    FOR EACH ROW EXECUTE FUNCTION bound_admin_media_page_intent_insert();

-- Metadata-only move intent. It is not proof of any filesystem observation,
-- move or deletion. The private executor performs and verifies those effects.
CREATE TABLE admin_media_quarantine_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    entries bigint NOT NULL DEFAULT 0 CHECK(entries>=0),
    bytes bigint NOT NULL DEFAULT 0 CHECK(bytes>=0),
    max_entries bigint NOT NULL DEFAULT 2000000,
    max_bytes bigint NOT NULL DEFAULT 536870912
);
INSERT INTO admin_media_quarantine_budget(singleton) VALUES(true);
CREATE TABLE admin_media_quarantine_intents (
    id uuid PRIMARY KEY,
    intent_id uuid NOT NULL,
    ordinal smallint NOT NULL,
    root_set_revision uuid NOT NULL REFERENCES admin_media_root_sets(revision) ON DELETE RESTRICT,
    maintenance_token uuid NOT NULL,
    reservation_token uuid NOT NULL,
    source_generation text NOT NULL CHECK(octet_length(source_generation) BETWEEN 1 AND 256),
    source_parent_generation text NOT NULL CHECK(octet_length(source_parent_generation) BETWEEN 1 AND 256),
    quarantine_root_role text NOT NULL CHECK(quarantine_root_role IN ('canonical','asset')),
    quarantine_parent_relative_hex text NOT NULL CHECK(length(quarantine_parent_relative_hex) BETWEEN 2 AND 262200 AND quarantine_parent_relative_hex ~ '^[0-9a-f]+$'),
    quarantine_parent_generation text NOT NULL CHECK(octet_length(quarantine_parent_generation) BETWEEN 1 AND 256),
    mount_id text NOT NULL CHECK(mount_id ~ '^[0-9a-f]{16}$'),
    leaf_name text NOT NULL CHECK(leaf_name ~ '^quarantine-[0-9a-f]{32}\.entry$'),
    logical_bytes integer NOT NULL CHECK(logical_bytes BETWEEN 1 AND 263224),
    state text NOT NULL CHECK(state IN ('intent','moved','uncertain','verified')),
    UNIQUE(intent_id,ordinal),
    FOREIGN KEY(intent_id,ordinal) REFERENCES admin_media_page_intent_entries(intent_id,ordinal) ON DELETE RESTRICT
);
CREATE INDEX admin_media_quarantine_intents_page_idx ON admin_media_quarantine_intents(intent_id,ordinal);
CREATE FUNCTION preserve_admin_media_quarantine_intent() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF OLD.state='verified' AND EXISTS(SELECT 1 FROM admin_media_page_intents i JOIN admin_media_completion_receipts c ON c.reservation_id=i.reservation_id WHERE i.id=OLD.intent_id AND c.cleanup_state='cleaning') THEN RETURN OLD; END IF;
        RAISE EXCEPTION 'quarantine intent requires explicit fenced recovery';
    END IF;
    IF (to_jsonb(NEW)-'state') IS DISTINCT FROM (to_jsonb(OLD)-'state') OR
       NOT ((OLD.state='intent' AND NEW.state IN ('intent','moved','uncertain')) OR
            (OLD.state='moved' AND NEW.state IN ('moved','uncertain','verified')) OR
            (OLD.state='uncertain' AND NEW.state='uncertain') OR
            (OLD.state='verified' AND NEW.state='verified')) THEN
        RAISE EXCEPTION 'quarantine intent binding is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_admin_media_quarantine_intent BEFORE UPDATE OR DELETE ON admin_media_quarantine_intents
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_quarantine_intent();

-- Frozen closure membership cannot grow after the coverage cursor has passed.
CREATE FUNCTION guard_frozen_admin_snapshot_insert() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_state text;
BEGIN
    SELECT state INTO current_state FROM admin_media_snapshots
        WHERE reservation_id=NEW.reservation_id FOR UPDATE;
    IF current_state IS DISTINCT FROM 'capturing' THEN
        RAISE EXCEPTION 'cannot append to a frozen media snapshot';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_frozen_admin_media_insert BEFORE INSERT ON admin_media_snapshot_entries
    FOR EACH ROW EXECUTE FUNCTION guard_frozen_admin_snapshot_insert();
CREATE TRIGGER guard_frozen_admin_object_insert BEFORE INSERT ON admin_media_snapshot_objects
    FOR EACH ROW EXECUTE FUNCTION guard_frozen_admin_snapshot_insert();

-- This bounded audit is bookkeeping only, not filesystem or deletion authority.
-- Its row count is bounded by the existing snapshot quota.
CREATE TABLE admin_media_coverage_audits (
    reservation_id uuid PRIMARY KEY REFERENCES admin_media_snapshots(reservation_id) ON DELETE RESTRICT,
    snapshot_revision uuid NOT NULL,
    phase text NOT NULL CHECK (phase IN ('media','objects','complete')),
    last_media uuid, last_kind text, last_object uuid,
    CHECK ((last_kind IS NULL) = (last_object IS NULL))
);

-- Created only by the future private capability-backed catalog transaction.
-- The record remains after bounded proof cleanup as the durable tombstone.
CREATE TABLE admin_media_completion_receipts (
    reservation_id uuid PRIMARY KEY REFERENCES admin_media_reservations(id) ON DELETE RESTRICT,
    completion_id uuid NOT NULL UNIQUE,
    snapshot_revision uuid NOT NULL,
    root_set_revision uuid NOT NULL,
    kind text NOT NULL CHECK(kind IN ('release','track','artist')),
    actor text NOT NULL CHECK(octet_length(actor) BETWEEN 1 AND 512),
    reason text NOT NULL CHECK(octet_length(reason) BETWEEN 1 AND 4096),
    cleanup_state text NOT NULL CHECK(cleanup_state IN ('pending','cleaning','done')),
    completed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE FUNCTION preserve_admin_media_completion() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR NEW.reservation_id<>OLD.reservation_id OR NEW.completion_id<>OLD.completion_id OR
       NEW.snapshot_revision<>OLD.snapshot_revision OR NEW.root_set_revision<>OLD.root_set_revision OR
       NEW.kind<>OLD.kind OR NEW.actor<>OLD.actor OR NEW.reason<>OLD.reason OR NEW.completed_at<>OLD.completed_at OR
       NOT ((OLD.cleanup_state='pending' AND NEW.cleanup_state IN ('pending','cleaning')) OR (OLD.cleanup_state='cleaning' AND NEW.cleanup_state IN ('cleaning','done')) OR (OLD.cleanup_state='done' AND NEW.cleanup_state='done')) THEN
        RAISE EXCEPTION 'completion tombstone is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_admin_media_completion BEFORE UPDATE OR DELETE ON admin_media_completion_receipts
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_completion();

-- Archive intents bind expected metadata only; they do not assert that any
-- archive file was created, persisted, or independently verified.
CREATE TABLE admin_media_archive_intents (
    reservation_id uuid PRIMARY KEY REFERENCES admin_media_completion_receipts(reservation_id) ON DELETE RESTRICT,
    completion_id uuid NOT NULL,
    maintenance_token uuid NOT NULL,
    reservation_token uuid NOT NULL,
    filename text NOT NULL CHECK(filename ~ '^admin-tombstone-[0-9a-f]{32}\.v1$'),
    lock_name text NOT NULL CHECK(lock_name ~ '^archive-[0-9a-f]{32}\.lock$'),
    lock_generation text CHECK(lock_generation IS NULL OR octet_length(lock_generation) BETWEEN 1 AND 256),
    byte_hash text NOT NULL CHECK(byte_hash ~ '^[0-9a-f]{64}$'),
    byte_size integer NOT NULL CHECK(byte_size BETWEEN 1 AND 8192),
    root_path_format text NOT NULL CHECK(root_path_format IN ('unix-hex-v1','windows-u16-hex-v1')),
    root_path_hex text NOT NULL CHECK(length(root_path_hex) BETWEEN 2 AND 131072 AND root_path_hex ~ '^[0-9a-f]+$'),
    root_generation text NOT NULL CHECK(octet_length(root_generation) BETWEEN 1 AND 256),
    root_mount_id text NOT NULL CHECK(root_mount_id ~ '^[0-9a-f]{16}$'),
    root_filesystem_magic text NOT NULL CHECK(root_filesystem_magic ~ '^[0-9a-f]{16}$'),
    state text NOT NULL CHECK(state IN ('intent','uncertain','archived'))
);
CREATE FUNCTION preserve_admin_media_archive_intent() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR (to_jsonb(NEW)-'state'-'lock_generation') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'lock_generation') OR
       (OLD.lock_generation IS NOT NULL AND NEW.lock_generation IS DISTINCT FROM OLD.lock_generation) OR
       NOT ((OLD.state='intent' AND NEW.state IN ('intent','uncertain','archived')) OR
            (OLD.state='uncertain' AND NEW.state='uncertain') OR (OLD.state='archived' AND NEW.state='archived')) THEN
        RAISE EXCEPTION 'archive intent binding is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_admin_media_archive_intent BEFORE UPDATE OR DELETE ON admin_media_archive_intents
    FOR EACH ROW EXECUTE FUNCTION preserve_admin_media_archive_intent();

CREATE FUNCTION guard_admin_delete_scope() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE scope_id uuid;
BEGIN
    scope_id := CASE WHEN TG_OP='DELETE' THEN OLD.reservation_id ELSE NEW.reservation_id END;
    -- The reservation row serializes this check with snapshot creation.
    PERFORM 1 FROM admin_media_reservations WHERE id=scope_id FOR UPDATE;
    IF TG_OP='UPDATE' OR EXISTS (SELECT 1 FROM admin_media_snapshots WHERE reservation_id=scope_id) THEN
        RAISE EXCEPTION 'bound deletion scope is immutable';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_admin_delete_scope BEFORE INSERT OR UPDATE OR DELETE ON admin_media_delete_scopes
    FOR EACH ROW EXECUTE FUNCTION guard_admin_delete_scope();
CREATE TRIGGER guard_admin_delete_target BEFORE INSERT OR UPDATE OR DELETE ON admin_media_delete_targets
    FOR EACH ROW EXECUTE FUNCTION guard_admin_delete_scope();

CREATE TABLE inbound_occurrence_purge_guards (
    work_item_id uuid PRIMARY KEY,
    source_library_id uuid,
    source_path text NOT NULL CHECK(octet_length(source_path) BETWEEN 1 AND 262200),
    guard_owner text NOT NULL CHECK(octet_length(guard_owner) BETWEEN 1 AND 512),
    fence_token uuid NOT NULL UNIQUE,
    intent_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX inbound_occurrence_purge_guards_path_idx ON inbound_occurrence_purge_guards(source_library_id,ingest_occurrence_hash(source_path));
CREATE INDEX inbound_retained_sources_active_path_idx ON inbound_retained_sources(source_library_id,ingest_occurrence_hash(source_path)) WHERE status='active';
CREATE INDEX inbound_retained_sources_ambiguous_path_idx ON inbound_retained_sources(source_library_id) WHERE status='active' AND source_path_format=0 AND strpos(source_path,chr(65533))>0;

CREATE FUNCTION inbound_occurrence_purge_guard_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM lock_inbound_occurrence(NEW.source_library_id,NEW.source_path);
    IF EXISTS (SELECT 1 FROM inbound_occurrence_purge_guards g
               WHERE g.source_library_id IS NOT DISTINCT FROM NEW.source_library_id
                 AND ingest_occurrence_hash(g.source_path)=ingest_occurrence_hash(NEW.source_path)
                 AND g.source_path COLLATE "C"=NEW.source_path COLLATE "C") THEN
        RAISE EXCEPTION 'purge intent already exists for occurrence' USING ERRCODE='55006';
    END IF;
    IF EXISTS (SELECT 1 FROM admin_media_reservation_items g WHERE g.source_library_id IS NOT DISTINCT FROM NEW.source_library_id
        AND ingest_occurrence_hash(g.source_path)=ingest_occurrence_hash(NEW.source_path) AND g.source_path COLLATE "C"=NEW.source_path COLLATE "C") THEN
        RAISE EXCEPTION 'purge blocked by administrative media reservation' USING ERRCODE='55006';
    END IF;
    IF EXISTS (SELECT 1 FROM inbound_retained_sources r
               WHERE r.status='active' AND r.source_library_id IS NOT DISTINCT FROM NEW.source_library_id
                 AND ((ingest_occurrence_hash(r.source_path)=ingest_occurrence_hash(NEW.source_path)
                       AND r.source_path COLLATE "C"=NEW.source_path COLLATE "C")
                      OR (r.source_path_format=0 AND strpos(r.source_path,chr(65533))>0))) THEN
        RAISE EXCEPTION 'purge blocked by retained source observation' USING ERRCODE='55006';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER inbound_occurrence_purge_guard_check BEFORE INSERT ON inbound_occurrence_purge_guards
    FOR EACH ROW EXECUTE FUNCTION inbound_occurrence_purge_guard_insert();
CREATE FUNCTION inbound_occurrence_purge_guard_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'purge guards cannot be rewritten';
END $$;
CREATE TRIGGER inbound_occurrence_purge_guard_immutable BEFORE UPDATE ON inbound_occurrence_purge_guards
    FOR EACH ROW EXECUTE FUNCTION inbound_occurrence_purge_guard_immutable();

CREATE FUNCTION inbound_retained_source_purge_interlock() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- A lossy old encoding cannot establish which native occurrence is held.
    IF NEW.source_path_format=0 AND strpos(NEW.source_path,chr(65533))>0 THEN
        RAISE EXCEPTION 'cannot create a retained source from a lossy ambiguous path' USING ERRCODE='55006';
    END IF;
    PERFORM lock_inbound_occurrence(NEW.source_library_id,NEW.source_path);
    IF NEW.baseline_id IS NOT NULL AND NOT EXISTS
       (SELECT 1 FROM inbound_retained_baselines b WHERE b.id=NEW.baseline_id AND b.work_item_id=NEW.work_item_id AND b.state='authoritative' AND b.protected_coverage AND b.stage_digest=NEW.content_digest) THEN
        RAISE EXCEPTION 'retained baseline binding is not authoritative' USING ERRCODE='55006';
    END IF;
    IF EXISTS (SELECT 1 FROM admin_media_reservation_items g WHERE g.source_library_id IS NOT DISTINCT FROM NEW.source_library_id
        AND ingest_occurrence_hash(g.source_path)=ingest_occurrence_hash(NEW.source_path) AND g.source_path COLLATE "C"=NEW.source_path COLLATE "C") THEN
        RAISE EXCEPTION 'retained source blocked by administrative media reservation' USING ERRCODE='55006';
    END IF;
    IF EXISTS (SELECT 1 FROM inbound_occurrence_purge_guards g
               WHERE g.source_library_id IS NOT DISTINCT FROM NEW.source_library_id
                 AND ingest_occurrence_hash(g.source_path)=ingest_occurrence_hash(NEW.source_path)
                 AND g.source_path COLLATE "C"=NEW.source_path COLLATE "C") THEN
        RAISE EXCEPTION 'retained source blocked by in-progress purge' USING ERRCODE='55006';
    END IF;
    IF EXISTS (SELECT 1 FROM release_work_items w JOIN release_work_item_staging_handoffs h ON h.work_item_id=w.id
               WHERE w.id<>NEW.work_item_id AND w.source_library_id IS NOT DISTINCT FROM NEW.source_library_id
                 AND ingest_occurrence_hash(w.source_path)=ingest_occurrence_hash(NEW.source_path)
                 AND w.source_path COLLATE "C"=NEW.source_path COLLATE "C"
                 AND h.status IN ('intent','staged')) THEN
        RAISE EXCEPTION 'retained source blocked by another active staging handoff' USING ERRCODE='55006';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER inbound_retained_source_purge_interlock BEFORE INSERT ON inbound_retained_sources
    FOR EACH ROW EXECUTE FUNCTION inbound_retained_source_purge_interlock();

-- A durable purge intent pins all path fields used to identify its filesystem
-- target until the fenced DB completion or explicit repair clears the guard.
CREATE FUNCTION inbound_guarded_work_item_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.source_library_id IS DISTINCT FROM OLD.source_library_id OR
        NEW.source_path COLLATE "C" IS DISTINCT FROM OLD.source_path COLLATE "C" OR
        NEW.current_path COLLATE "C" IS DISTINCT FROM OLD.current_path COLLATE "C") AND
       (EXISTS (SELECT 1 FROM inbound_occurrence_purge_guards WHERE work_item_id=OLD.id) OR
        EXISTS (SELECT 1 FROM admin_media_reservation_items WHERE work_item_id=OLD.id)) THEN
        RAISE EXCEPTION 'purge intent pins occurrence and current path' USING ERRCODE='55006';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER inbound_guarded_work_item_binding BEFORE UPDATE OF source_library_id,source_path,current_path ON release_work_items
    FOR EACH ROW EXECUTE FUNCTION inbound_guarded_work_item_binding();

-- Protected-stage reservations never contain media bytes. A reservation with
-- unknown filesystem outcome remains durable/quarantined and consumes quota;
-- automatic expiry would permit unsafe adoption or deletion after a crash.
CREATE TABLE inbound_protected_attempt_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    max_attempts bigint NOT NULL DEFAULT 4096 CHECK(max_attempts>0),
    max_bytes bigint NOT NULL DEFAULT 67108864 CHECK(max_bytes>0),
    attempts bigint NOT NULL DEFAULT 0 CHECK(attempts>=0),
    bytes bigint NOT NULL DEFAULT 0 CHECK(bytes>=0)
);
INSERT INTO inbound_protected_attempt_budget(singleton) VALUES(true);
CREATE TABLE inbound_protected_stage_attempts (
    id uuid PRIMARY KEY,
    work_item_id uuid NOT NULL,
    owner text NOT NULL CHECK(octet_length(owner) BETWEEN 1 AND 512),
    claim_epoch uuid NOT NULL,
    source_library_id uuid,
    source_path text NOT NULL CHECK(octet_length(source_path) BETWEEN 1 AND 262200),
    source_path_format smallint NOT NULL CHECK(source_path_format IN (0,1)),
    source_generation text NOT NULL CHECK(octet_length(source_generation) BETWEEN 1 AND 512),
    root_generation text NOT NULL CHECK(octet_length(root_generation) BETWEEN 1 AND 512),
    final_binding text NOT NULL CHECK(octet_length(final_binding) BETWEEN 1 AND 262200),
    namespace_name text NOT NULL UNIQUE CHECK(namespace_name ~ '^protected-[0-9a-f]{32}$'),
    state text NOT NULL CHECK(state IN ('reserved','created','promotion_intent','promoted','consumed','cleanup_intent')),
    promotion_content_generation text CHECK(promotion_content_generation IS NULL OR octet_length(promotion_content_generation) BETWEEN 1 AND 512),
    promotion_parent_generation text CHECK(promotion_parent_generation IS NULL OR octet_length(promotion_parent_generation) BETWEEN 1 AND 512),
    promoted_generation text CHECK(promoted_generation IS NULL OR octet_length(promoted_generation) BETWEEN 1 AND 512),
    consumed_baseline_id uuid,
    consumed_session_id uuid,
    consumed_legacy_hash text CHECK(consumed_legacy_hash IS NULL OR consumed_legacy_hash ~ '^[0-9a-f]{64}$'),
    consumed_source_digest text CHECK(consumed_source_digest IS NULL OR consumed_source_digest ~ '^v3:[0-9a-f]{64}$'),
    namespace_generation text CHECK(namespace_generation IS NULL OR octet_length(namespace_generation) BETWEEN 1 AND 512),
    marker_generation text CHECK(marker_generation IS NULL OR octet_length(marker_generation) BETWEEN 1 AND 512),
    lock_generation text CHECK(lock_generation IS NULL OR octet_length(lock_generation) BETWEEN 1 AND 512),
    cleanup_owner text CHECK(cleanup_owner IS NULL OR octet_length(cleanup_owner) BETWEEN 1 AND 512),
    cleanup_token uuid,
    cleanup_expires_at timestamptz,
    cleanup_due_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    cleanup_retry_count integer NOT NULL DEFAULT 0 CHECK(cleanup_retry_count>=0),
    cleanup_last_reason text CHECK(cleanup_last_reason IS NULL OR cleanup_last_reason IN ('busy','missing','changed','unreadable')),
    logical_bytes bigint NOT NULL CHECK(logical_bytes>0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK ((cleanup_owner IS NULL AND cleanup_token IS NULL AND cleanup_expires_at IS NULL) OR
           (state IN ('consumed','cleanup_intent') AND cleanup_owner IS NOT NULL AND cleanup_token IS NOT NULL AND cleanup_expires_at IS NOT NULL)),
    CHECK (((state IN ('consumed','cleanup_intent') AND consumed_baseline_id IS NOT NULL AND consumed_session_id IS NOT NULL AND consumed_legacy_hash IS NOT NULL AND consumed_source_digest IS NOT NULL) OR (state NOT IN ('consumed','cleanup_intent') AND consumed_baseline_id IS NULL AND consumed_session_id IS NULL AND consumed_legacy_hash IS NULL AND consumed_source_digest IS NULL)) IS TRUE),
    CHECK (((state='reserved' AND namespace_generation IS NULL AND marker_generation IS NULL AND lock_generation IS NULL AND promotion_content_generation IS NULL AND promotion_parent_generation IS NULL AND promoted_generation IS NULL) OR
           (state='created' AND namespace_generation IS NOT NULL AND marker_generation IS NOT NULL AND lock_generation IS NOT NULL AND promotion_content_generation IS NULL AND promotion_parent_generation IS NULL AND promoted_generation IS NULL) OR
           (state='promotion_intent' AND namespace_generation IS NOT NULL AND marker_generation IS NOT NULL AND lock_generation IS NOT NULL AND promotion_content_generation IS NOT NULL AND promotion_parent_generation IS NOT NULL AND promoted_generation IS NULL) OR
           (state IN ('promoted','consumed','cleanup_intent') AND namespace_generation IS NOT NULL AND marker_generation IS NOT NULL AND lock_generation IS NOT NULL AND promotion_content_generation IS NOT NULL AND promotion_parent_generation IS NOT NULL AND promoted_generation=promotion_content_generation)) IS TRUE)
);
CREATE INDEX inbound_protected_stage_attempts_item_idx ON inbound_protected_stage_attempts(work_item_id,id);
CREATE INDEX inbound_protected_stage_attempts_cleanup_idx ON inbound_protected_stage_attempts(cleanup_due_at,id) WHERE state IN ('consumed','cleanup_intent');
CREATE FUNCTION inbound_protected_stage_attempt_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id<>OLD.id OR NEW.work_item_id<>OLD.work_item_id OR
       NEW.source_library_id IS DISTINCT FROM OLD.source_library_id OR
       NEW.source_path COLLATE "C"<>OLD.source_path COLLATE "C" OR NEW.source_path_format<>OLD.source_path_format OR
       NEW.source_generation<>OLD.source_generation OR NEW.root_generation<>OLD.root_generation OR
       NEW.final_binding COLLATE "C"<>OLD.final_binding COLLATE "C" OR NEW.namespace_name<>OLD.namespace_name OR
       NEW.logical_bytes<>OLD.logical_bytes OR
       (OLD.state<>'reserved' AND (NEW.namespace_generation<>OLD.namespace_generation OR NEW.marker_generation<>OLD.marker_generation OR NEW.lock_generation<>OLD.lock_generation)) OR
       (OLD.state IN ('promotion_intent','promoted','consumed','cleanup_intent') AND (NEW.promotion_content_generation<>OLD.promotion_content_generation OR NEW.promotion_parent_generation<>OLD.promotion_parent_generation)) OR
       (OLD.state IN ('consumed','cleanup_intent') AND (NEW.consumed_baseline_id IS DISTINCT FROM OLD.consumed_baseline_id OR NEW.consumed_session_id IS DISTINCT FROM OLD.consumed_session_id OR NEW.consumed_legacy_hash IS DISTINCT FROM OLD.consumed_legacy_hash OR NEW.consumed_source_digest IS DISTINCT FROM OLD.consumed_source_digest)) OR
       NOT ((OLD.state='reserved' AND NEW.state IN ('reserved','created')) OR
            (OLD.state='created' AND NEW.state IN ('created','promotion_intent')) OR
            (OLD.state='promotion_intent' AND NEW.state IN ('promotion_intent','promoted')) OR
            (OLD.state='promoted' AND NEW.state IN ('promoted','consumed')) OR
            (OLD.state='consumed' AND NEW.state IN ('consumed','cleanup_intent')) OR
            (OLD.state='cleanup_intent' AND NEW.state='cleanup_intent')) THEN
       RAISE EXCEPTION 'protected stage attempt identity is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER inbound_protected_stage_attempt_immutable BEFORE UPDATE ON inbound_protected_stage_attempts
    FOR EACH ROW EXECUTE FUNCTION inbound_protected_stage_attempt_immutable();
