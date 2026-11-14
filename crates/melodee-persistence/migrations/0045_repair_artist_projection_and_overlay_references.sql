-- ADR-0194: migration 0044 changed an indexed value without a revision/outbox.
-- Refresh every live Artist, also covering missed aggregate updates from name reuse.
-- Preserve every public ID, status, historical decision and immutable overlay.
WITH advanced AS (
    UPDATE artists SET revision = revision + 1, updated_at = now()
     WHERE status IN ('active', 'verified')
     RETURNING id, revision
)
INSERT INTO catalog_projection_outbox (
    projection_name, entity_type, entity_id, operation, entity_revision,
    available_at, attempt_count, created_at
)
SELECT 'tantivy.catalog', 'artist', id, 'upsert', revision, now(), 0, now()
  FROM advanced;

-- A one-time SQL inventory avoids rescanning every overlay on later starts.
-- The Rust migration runner drains this bounded queue using the domain's exact
-- canonical hash algorithm. A committed replacement and queue removal are atomic;
-- a crash leaves the remaining records available for the next startup.
CREATE TABLE catalog_overlay_repair_queue (
    release_id uuid PRIMARY KEY REFERENCES releases (id) ON DELETE RESTRICT,
    invalid_overlay_id uuid NOT NULL,
    FOREIGN KEY (invalid_overlay_id, release_id)
        REFERENCES catalog_semantic_overlays (id, release_id) ON DELETE RESTRICT
);

INSERT INTO catalog_overlay_repair_queue (release_id, invalid_overlay_id)
SELECT release.id, overlay.id
  FROM releases release
  JOIN catalog_semantic_overlays overlay ON overlay.id = release.current_overlay_id
 WHERE EXISTS (
    SELECT 1
      FROM jsonb_array_elements(overlay.payload->'references'->'artist_resolutions') reference
      LEFT JOIN artist_match_decisions decision
        ON decision.id::text = reference->>'decision_id'
      LEFT JOIN artists artist ON artist.id::text = reference->>'artist_id'
     WHERE artist.id IS NULL
        OR reference->>'artist_id' IS DISTINCT FROM decision.artist_id::text
 );
