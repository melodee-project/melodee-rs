-- ADR-0214: bounded processed search-outbox history.
--
-- Successful outbox detail is a seven-day operational window, while the Admin
-- activity chart needs exact distinct-Track counts for 400 days. This
-- migration adds the durable hourly aggregate, the per-projection maintenance
-- watermark, and the bounded tombstone that keeps producer idempotency when an
-- old unique outbox row has been deleted.

CREATE TABLE search_outbox_hourly_totals (
    projection_name text NOT NULL,
    hour timestamptz NOT NULL,
    indexed_tracks bigint NOT NULL,
    finalized_at timestamptz NOT NULL,

    CONSTRAINT search_outbox_hourly_totals_projection_check CHECK (
        btrim(projection_name) <> ''
    ),
    CONSTRAINT search_outbox_hourly_totals_hour_check CHECK (
        date_trunc('hour', hour) = hour
    ),
    CONSTRAINT search_outbox_hourly_totals_count_check CHECK (indexed_tracks >= 0),
    PRIMARY KEY (projection_name, hour)
);

CREATE INDEX search_outbox_hourly_totals_age_idx
    ON search_outbox_hourly_totals (projection_name, hour);

-- One maintenance record per projection. `highest_pruned_id` is written in the
-- same transaction as each delete batch, so a reader that needs a high-water
-- mark can distinguish "no rows above this id" from "rows were pruned".
CREATE TABLE search_outbox_maintenance (
    projection_name text PRIMARY KEY,
    finalized_through timestamptz NULL,
    earliest_trustworthy_hour timestamptz NULL,
    highest_pruned_id bigint NOT NULL DEFAULT 0,
    last_prune_at timestamptz NULL,
    last_prune_result jsonb NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT search_outbox_maintenance_projection_check CHECK (
        btrim(projection_name) <> ''
    ),
    CONSTRAINT search_outbox_maintenance_high_water_check CHECK (
        highest_pruned_id >= 0
    ),
    CONSTRAINT search_outbox_maintenance_finalized_check CHECK (
        finalized_through IS NULL OR date_trunc('hour', finalized_through) = finalized_through
    )
);

-- Bounded duplicate suppression: one row per projection/entity with the
-- highest pruned revision. Replaying a stale upsert for an already-pruned
-- revision is suppressed by the trigger below, so an outbox delete does not
-- resurrect old work. One row per entity, never one row per event.
CREATE TABLE search_outbox_prune_tombstones (
    projection_name text NOT NULL,
    entity_type text NOT NULL,
    entity_id uuid NOT NULL,
    pruned_through_revision bigint NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT search_outbox_prune_tombstones_projection_check CHECK (
        btrim(projection_name) <> '' AND btrim(entity_type) <> ''
    ),
    CONSTRAINT search_outbox_prune_tombstones_revision_check CHECK (
        pruned_through_revision > 0
    ),
    PRIMARY KEY (projection_name, entity_type, entity_id)
);

CREATE FUNCTION suppress_pruned_outbox_replay() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM search_outbox_prune_tombstones tombstone
         WHERE tombstone.projection_name = NEW.projection_name
           AND tombstone.entity_type = NEW.entity_type
           AND tombstone.entity_id = NEW.entity_id
           AND tombstone.pruned_through_revision >= NEW.entity_revision
    ) THEN
        RETURN NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER catalog_projection_outbox_pruned_replay
BEFORE INSERT ON catalog_projection_outbox
FOR EACH ROW EXECUTE FUNCTION suppress_pruned_outbox_replay();

-- Pruning selects processed rows by age; the partial index keeps the scan off
-- pending, leased and failed work.
CREATE INDEX catalog_projection_outbox_processed_age_idx
    ON catalog_projection_outbox (projection_name, processed_at, id)
    WHERE processed_at IS NOT NULL;
