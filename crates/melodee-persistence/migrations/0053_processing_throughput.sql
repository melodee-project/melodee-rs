-- Additive. Existing and unmarked future arrivals use the ordinary lane.
CREATE TABLE ingest_bulk_submissions (
    id uuid PRIMARY KEY,
    registered_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE ingest_bulk_entries (
    batch_id uuid NOT NULL REFERENCES ingest_bulk_submissions,
    source_id uuid,
    source_path text NOT NULL CHECK (source_path NOT IN ('', '.', '..') AND strpos(source_path, '/') = 0),
    work_item_id uuid UNIQUE,
    PRIMARY KEY (batch_id, source_path)
);
CREATE UNIQUE INDEX ingest_bulk_next_occurrence
    ON ingest_bulk_entries (source_path, source_id) NULLS NOT DISTINCT WHERE work_item_id IS NULL;
ALTER TABLE release_work_items ADD COLUMN bulk_submission_id uuid REFERENCES ingest_bulk_submissions;
ALTER TABLE release_work_items ADD COLUMN stage_entered_at timestamptz;

CREATE FUNCTION ingest_processing_stage(s text) RETURNS text
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
    SELECT CASE
        WHEN s IN ('discovered','settling') THEN 'intake'
        WHEN s IN ('ready','inspecting','failed') THEN 'inspection'
        WHEN s = 'normalizing' THEN 'normalization'
        WHEN s = 'validating' THEN 'validation'
        WHEN s IN ('staged','publishing') THEN 'publication'
        WHEN s IN ('published','cataloging') THEN 'catalog'
        WHEN s = 'cataloged' THEN 'policy'
        WHEN s = 'indexing' THEN 'indexing'
    END
$$;

-- One pass over historical transitions. Heartbeats must never reset queue age.
WITH entered AS (
    SELECT DISTINCT ON (work_item_id) work_item_id, created_at
    FROM release_work_item_events
    WHERE to_state IS NOT NULL AND ingest_processing_stage(from_state) IS DISTINCT FROM ingest_processing_stage(to_state)
    ORDER BY work_item_id, created_at DESC, id DESC
)
UPDATE release_work_items w SET stage_entered_at = e.created_at
FROM entered e WHERE e.work_item_id = w.id;
CREATE FUNCTION ingest_track_stage() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.stage_entered_at := NEW.discovered_at;
    ELSIF ingest_processing_stage(OLD.state) IS DISTINCT FROM ingest_processing_stage(NEW.state) THEN
        NEW.stage_entered_at := NEW.updated_at;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER ingest_track_stage BEFORE INSERT OR UPDATE OF state ON release_work_items
FOR EACH ROW EXECUTE FUNCTION ingest_track_stage();

CREATE TABLE ingest_claim_cursors (
    stage text PRIMARY KEY, slot integer NOT NULL DEFAULT 0 CHECK (slot BETWEEN 0 AND 3)
);
INSERT INTO ingest_claim_cursors(stage) VALUES
    ('inspection'), ('normalization'), ('validation'), ('publication'), ('catalog'), ('policy');
CREATE INDEX ingest_priority_queue ON release_work_items
    (ingest_processing_stage(state), (bulk_submission_id IS NOT NULL),
     (CASE WHEN state='failed' AND last_error_code='lease.lost' THEN 0 ELSE 1 END),
     (CASE WHEN ingest_processing_stage(state)='inspection' THEN next_attempt_at END) NULLS FIRST,
     (CASE WHEN ingest_processing_stage(state)='inspection' THEN created_at ELSE updated_at END), id)
    WHERE ingest_processing_stage(state) IS NOT NULL;
CREATE INDEX ingest_arrival_window ON release_work_items(discovered_at);
CREATE INDEX ingest_stage_completions ON release_work_item_events(ingest_processing_stage(from_state), created_at DESC) INCLUDE (to_state)
    WHERE from_state IS NOT NULL AND to_state IS NOT NULL AND from_state <> to_state;

-- Keep eligibility shared between claiming and diagnostics. Inspection recovery
-- explicitly transitions an expired inspecting lease before it can be claimed.
CREATE VIEW ingest_runnable AS
SELECT w.*, ingest_processing_stage(state) AS stage
FROM release_work_items w
WHERE state IN ('ready','inspecting','normalizing','validating','indexing','discovered','settling')
   OR (state = 'failed' AND next_attempt_at IS NOT NULL)
   OR (state IN ('staged','publishing') AND EXISTS
       (SELECT FROM release_publication_plans p WHERE p.work_item_id=w.id))
   OR (state IN ('published','cataloging') AND EXISTS
       (SELECT FROM release_publication_receipts r WHERE r.work_item_id=w.id))
   OR (state='cataloged' AND EXISTS (SELECT FROM catalog_imports c WHERE c.work_item_id=w.id AND c.release_id IS NOT NULL));

-- Preserve the first automated outcome. Later manual intervention cannot
-- retroactively change the observation window of a production benchmark.
ALTER TABLE release_work_items ADD COLUMN first_processing_outcome text,
    ADD COLUMN first_processing_outcome_at timestamptz;
CREATE FUNCTION ingest_record_outcome() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.first_processing_outcome := CASE
        WHEN NEW.state='indexing' AND NEW.state_reason='policy.disposition.accept' THEN 'playable'
        WHEN NEW.state IN ('needs_review','quarantined') THEN 'held'
        WHEN NEW.state='failed' AND NEW.next_attempt_at IS NULL THEN 'failed'
        WHEN NEW.state IN ('ignored','deleted') THEN 'excluded'
    END;
    IF NEW.first_processing_outcome IS NOT NULL THEN
        NEW.first_processing_outcome_at := NEW.updated_at;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER ingest_record_outcome BEFORE UPDATE OF state,next_attempt_at ON release_work_items
FOR EACH ROW WHEN (OLD.first_processing_outcome IS NULL) EXECUTE FUNCTION ingest_record_outcome();


-- Purging a failed work item must not remove it from a submission's denominator.
-- Keep only timing and outcome, without paths, manifests or failure messages.
CREATE TABLE ingest_purged_arrivals (
    id uuid PRIMARY KEY,
    bulk_submission_id uuid REFERENCES ingest_bulk_submissions,
    discovered_at timestamptz NOT NULL,
    outcome text NOT NULL,
    finished_at timestamptz NOT NULL
);
CREATE INDEX ingest_purged_window ON ingest_purged_arrivals(discovered_at);
CREATE FUNCTION ingest_preserve_outcome_on_purge() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.bulk_submission_id IS NOT NULL OR EXISTS (
        SELECT FROM ingest_bulk_submissions WHERE registered_at <= OLD.discovered_at
    ) THEN
        INSERT INTO ingest_purged_arrivals VALUES (OLD.id,OLD.bulk_submission_id,OLD.discovered_at,
            coalesce(OLD.first_processing_outcome,'excluded'),coalesce(OLD.first_processing_outcome_at,now()));
    END IF;
    RETURN OLD;
END
$$;
CREATE TRIGGER ingest_preserve_outcome_on_purge BEFORE DELETE ON release_work_items
FOR EACH ROW EXECUTE FUNCTION ingest_preserve_outcome_on_purge();
