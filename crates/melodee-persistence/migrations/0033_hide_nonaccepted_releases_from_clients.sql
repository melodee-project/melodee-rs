-- Catalog import precedes policy evaluation, so a Release can have durable
-- catalog rows while policy parks it outside the accepted path. Client APIs
-- already require releases.status = 'active'; align existing non-accepted
-- policy outcomes with that shared visibility boundary.
--
-- This changes no media, catalog identity, or work history. A later audited
-- Accept override restores status='active' transactionally in PolicyStore.
UPDATE releases release
   SET status = 'deprecated',
       revision = revision + 1,
       updated_at = clock_timestamp()
  FROM catalog_imports import
  JOIN release_work_items item ON item.id = import.work_item_id
 WHERE import.release_id = release.id
   AND release.status = 'active'
   AND item.state IN ('needs_review', 'quarantined', 'ignored', 'deleted');
