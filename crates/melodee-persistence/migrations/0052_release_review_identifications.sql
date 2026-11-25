-- Only an authenticated operator can select a match. Keep the choice outside
-- the manifest: imported tags are observations, never authority to merge.
CREATE TABLE release_review_identifications (
    work_item_id uuid PRIMARY KEY REFERENCES release_work_items(id) ON DELETE CASCADE,
    provider text NOT NULL,
    external_id text NOT NULL CHECK (btrim(external_id) <> ''),
    target_release_id uuid REFERENCES releases(id) ON DELETE RESTRICT,
    actor uuid NOT NULL,
    applied_at timestamptz,
    merge_event_id uuid REFERENCES release_merge_events(id),
    CHECK ((provider = 'melodee') = (target_release_id IS NOT NULL))
);
CREATE INDEX release_review_identifications_pending ON release_review_identifications(work_item_id)
    WHERE applied_at IS NULL;
