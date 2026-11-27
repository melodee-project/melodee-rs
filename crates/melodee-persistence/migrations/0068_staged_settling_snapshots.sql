-- Captures are written directly to durable snapshot evidence in bounded pages.
-- A scope may have one writable capture; discarded captures drain independently.
ALTER TABLE release_work_item_settling_snapshots
    ADD COLUMN capture_complete boolean NOT NULL DEFAULT true,
    ADD COLUMN capture_frozen boolean NOT NULL DEFAULT false,
    ADD COLUMN capture_discarding boolean NOT NULL DEFAULT false,
    ADD COLUMN capture_scope text,
    ADD COLUMN capture_generation uuid,
    ADD COLUMN capture_owner uuid,
    ADD COLUMN capture_source_path text,
    ADD COLUMN capture_source_state text,
    ADD COLUMN capture_cleanup_at timestamptz,
    ADD CONSTRAINT release_settling_capture_binding_check CHECK (
        capture_complete OR
        (capture_scope IS NOT NULL AND capture_generation IS NOT NULL AND
         capture_owner IS NOT NULL AND capture_source_path IS NOT NULL AND capture_source_state IS NOT NULL)
    );

CREATE UNIQUE INDEX release_settling_one_incomplete_capture_per_scope
    ON release_work_item_settling_snapshots (capture_scope)
    WHERE NOT capture_complete AND NOT capture_discarding;
CREATE INDEX release_settling_discarding_capture_idx
    ON release_work_item_settling_snapshots (capture_cleanup_at NULLS FIRST, id)
    WHERE NOT capture_complete AND capture_discarding;

-- PostgreSQL timestamps retain microseconds, while signature v2 includes the
-- original nanoseconds. Existing finalized evidence needs no conversion.
ALTER TABLE release_work_item_settling_snapshot_files
    ADD COLUMN modified_at_nanos text;

CREATE INDEX release_settling_capture_files_byte_order_idx
    ON release_work_item_settling_snapshot_files (snapshot_id, relative_path COLLATE "C");

-- Limits are logical staged metadata, not audio payload bytes or a promise
-- about PostgreSQL heap/index/WAL overhead. Operators can raise them before
-- retrying a Release larger than the configured budget.
CREATE TABLE ingestion_settling_capture_budget (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    max_captures bigint NOT NULL DEFAULT 4096 CHECK (max_captures > 0),
    active_captures bigint NOT NULL DEFAULT 0 CHECK (active_captures >= 0),
    max_rows bigint NOT NULL DEFAULT 1000000 CHECK (max_rows > 0),
    max_bytes bigint NOT NULL DEFAULT 1073741824 CHECK (max_bytes > 0),
    active_rows bigint NOT NULL DEFAULT 0 CHECK (active_rows >= 0),
    active_bytes bigint NOT NULL DEFAULT 0 CHECK (active_bytes >= 0)
);
INSERT INTO ingestion_settling_capture_budget (singleton) VALUES (true);

ALTER TABLE release_work_item_settling_snapshots
    ADD COLUMN capture_rows bigint NOT NULL DEFAULT 0 CHECK (capture_rows >= 0),
    ADD COLUMN capture_bytes bigint NOT NULL DEFAULT 0 CHECK (capture_bytes >= 0),
    ADD COLUMN capture_file_bytes bigint NOT NULL DEFAULT 0 CHECK (capture_file_bytes >= 0);

-- Empty captures also consume storage. Account for and cap their count,
-- otherwise a stream of empty discarded observations bypasses the row limit.
CREATE FUNCTION settling_capture_admit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT NEW.capture_complete THEN
        UPDATE ingestion_settling_capture_budget SET active_captures=active_captures+1,
            active_bytes=active_bytes+512 WHERE singleton AND active_captures<max_captures
            AND 512 <= max_bytes-active_bytes;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'settling capture budget exhausted; raise limits and retry' USING ERRCODE='53400';
        END IF;
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER settling_capture_admit BEFORE INSERT ON release_work_item_settling_snapshots
    FOR EACH ROW EXECUTE FUNCTION settling_capture_admit();

-- Aggregate once per bounded page instead of rewriting the budget and parent
-- snapshot once for every file in the page.
CREATE FUNCTION settling_capture_file_accounting() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    page record;
    parent_complete boolean;
BEGIN
    IF TG_OP = 'INSERT' THEN
        FOR page IN SELECT snapshot_id, count(*)::bigint AS row_count,
                   sum(octet_length(relative_path)::bigint + octet_length(file_type)::bigint +
                       COALESCE(octet_length(modified_at_nanos),0)::bigint + 128)::bigint AS byte_count,
                   sum(file_size)::bigint AS media_bytes
              FROM inserted_files GROUP BY snapshot_id ORDER BY snapshot_id LOOP
            SELECT capture_complete INTO parent_complete FROM release_work_item_settling_snapshots
                WHERE id=page.snapshot_id FOR UPDATE;
            IF FOUND AND NOT parent_complete THEN
                UPDATE ingestion_settling_capture_budget SET active_rows=active_rows+page.row_count,
                    active_bytes=active_bytes+page.byte_count WHERE singleton
                    AND page.row_count <= max_rows-active_rows AND page.byte_count <= max_bytes-active_bytes;
                IF NOT FOUND THEN
                    RAISE EXCEPTION 'settling capture budget exhausted; raise limits and retry' USING ERRCODE='53400';
                END IF;
                UPDATE release_work_item_settling_snapshots SET capture_rows=capture_rows+page.row_count,
                    capture_bytes=capture_bytes+page.byte_count,capture_file_bytes=capture_file_bytes+page.media_bytes
                    WHERE id=page.snapshot_id;
            END IF;
        END LOOP;
    ELSE
        FOR page IN SELECT snapshot_id, count(*)::bigint AS row_count,
                   sum(octet_length(relative_path)::bigint + octet_length(file_type)::bigint +
                       COALESCE(octet_length(modified_at_nanos),0)::bigint + 128)::bigint AS byte_count,
                   sum(file_size)::bigint AS media_bytes
              FROM deleted_files GROUP BY snapshot_id ORDER BY snapshot_id LOOP
            SELECT capture_complete INTO parent_complete FROM release_work_item_settling_snapshots
                WHERE id=page.snapshot_id FOR UPDATE;
            -- ON DELETE CASCADE sees the parent already removed: its delete
            -- trigger released the whole capture; do not release it twice.
            IF FOUND AND NOT parent_complete THEN
                UPDATE ingestion_settling_capture_budget SET active_rows=active_rows-page.row_count,
                    active_bytes=active_bytes-page.byte_count WHERE singleton;
                UPDATE release_work_item_settling_snapshots SET capture_rows=capture_rows-page.row_count,
                    capture_bytes=capture_bytes-page.byte_count,capture_file_bytes=capture_file_bytes-page.media_bytes
                    WHERE id=page.snapshot_id;
            END IF;
        END LOOP;
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER settling_capture_file_insert_accounting
    AFTER INSERT ON release_work_item_settling_snapshot_files
    REFERENCING NEW TABLE AS inserted_files FOR EACH STATEMENT EXECUTE FUNCTION settling_capture_file_accounting();
CREATE TRIGGER settling_capture_file_delete_accounting
    AFTER DELETE ON release_work_item_settling_snapshot_files
    REFERENCING OLD TABLE AS deleted_files FOR EACH STATEMENT EXECUTE FUNCTION settling_capture_file_accounting();

CREATE FUNCTION settling_capture_release_accounting() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF NOT OLD.capture_complete THEN
            UPDATE ingestion_settling_capture_budget SET active_rows=active_rows-OLD.capture_rows,
                active_bytes=active_bytes-OLD.capture_bytes-512,
                active_captures=active_captures-1 WHERE singleton;
        END IF;
        RETURN OLD;
    END IF;
    IF NOT OLD.capture_complete AND NEW.capture_complete THEN
        UPDATE ingestion_settling_capture_budget SET active_rows=active_rows-OLD.capture_rows,
            active_bytes=active_bytes-OLD.capture_bytes-512,
                active_captures=active_captures-1 WHERE singleton;
        NEW.capture_rows := 0;
        NEW.capture_bytes := 0;
        NEW.capture_file_bytes := 0;
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER settling_capture_release_accounting
    BEFORE UPDATE OR DELETE ON release_work_item_settling_snapshots
    FOR EACH ROW EXECUTE FUNCTION settling_capture_release_accounting();
