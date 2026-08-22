-- Normalization provenance and claim index.
--
-- The manifest_payload JSONB already carries the full NormalizationObservations
-- including volatile provenance. This migration adds a dedicated provenance
-- column so the worker can record execution metadata (tool paths, elapsed time,
-- worker identity) without rewriting the entire manifest payload, and adds an
-- index to efficiently claim work items in the 'normalizing' state.

ALTER TABLE release_work_item_manifests
    ADD COLUMN normalization_provenance jsonb NULL,
    ADD COLUMN normalization_started_at timestamptz NULL,
    ADD COLUMN normalization_completed_at timestamptz NULL;

-- Index for claiming normalizing items: pending (no active lease) normalizing rows
-- are the normalization worker's candidate set. The predicate uses only
-- immutable expressions (lease_expires_at IS NULL or comparison to a constant
-- is not needed - the worker re-checks lease expiry at lock time).
CREATE INDEX release_work_items_normalizing_claim_idx
    ON release_work_items (updated_at, id)
    WHERE state = 'normalizing' AND lease_expires_at IS NULL;
