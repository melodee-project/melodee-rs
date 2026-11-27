SELECT id,state,CASE WHEN stage='policy' THEN
    (SELECT c.release_id FROM catalog_imports c WHERE c.work_item_id=w.id) END AS release_id
FROM ingest_runnable w
WHERE stage = $1 AND state <> 'inspecting' AND (bulk_submission_id IS NOT NULL) = $2
  AND ($1 <> 'inspection' OR NOT EXISTS (
      SELECT 1 FROM release_work_items occurrence WHERE occurrence.id=w.id
        AND occurrence.discovery_path_format=0 AND strpos(occurrence.source_path, chr(65533))>0
        AND NOT EXISTS (SELECT 1 FROM release_work_item_staging_handoffs handoff
            WHERE handoff.work_item_id=occurrence.id AND handoff.status IN ('staged','inbound_removed'))))
  AND (lease_expires_at IS NULL OR lease_expires_at <= $3)
  AND (next_attempt_at IS NULL OR next_attempt_at <= $3)
  AND (state <> 'failed' OR attempt_count < $4)
ORDER BY CASE WHEN state='failed' AND last_error_code='lease.lost' THEN 0 ELSE 1 END,
         CASE WHEN stage='inspection' THEN next_attempt_at END NULLS FIRST,
         CASE WHEN stage='inspection' THEN created_at ELSE updated_at END, id
LIMIT $5 FOR UPDATE SKIP LOCKED
