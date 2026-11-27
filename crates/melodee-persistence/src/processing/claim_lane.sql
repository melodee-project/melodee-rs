SELECT id,state,source_library_id,source_path,CASE WHEN stage='policy' THEN
    (SELECT c.release_id FROM catalog_imports c WHERE c.work_item_id=w.id) END AS release_id
FROM ingest_runnable w
WHERE stage = $1 AND state <> 'inspecting'
  AND (NOT (SELECT enforcement_enabled FROM work_item_lifecycle_maintenance WHERE singleton)
       OR EXISTS (SELECT 1 FROM work_item_lifecycle_locks l JOIN release_work_items source ON source.id=l.work_item_id
                  WHERE l.work_item_id=w.id AND l.state='ready' AND NOT EXISTS (SELECT 1 FROM work_item_lifecycle_quarantine q WHERE q.work_item_id=l.work_item_id)
                    AND l.source_library_id IS NOT DISTINCT FROM source.source_library_id
                    AND l.source_path COLLATE "C"=source.source_path COLLATE "C"
                    AND l.source_path_format=source.discovery_path_format))
  AND ($6 OR NOT EXISTS (SELECT 1 FROM release_work_items protected_item WHERE protected_item.id=w.id AND protected_item.protected_custody_required)) AND (bulk_submission_id IS NOT NULL) = $2
  AND ($1 <> 'inspection' OR NOT EXISTS (
      SELECT 1 FROM release_work_items occurrence WHERE occurrence.id=w.id
        AND occurrence.discovery_path_format=0 AND strpos(occurrence.source_path, chr(65533))>0
        AND NOT EXISTS (SELECT 1 FROM release_work_item_staging_handoffs handoff
            WHERE handoff.work_item_id=occurrence.id AND handoff.status IN ('staged','inbound_removed'))))
  AND (lease_expires_at IS NULL OR lease_expires_at <= $3)
  AND (next_attempt_at IS NULL OR next_attempt_at <= $3)
  AND (state <> 'failed' OR attempt_count < $4)
  AND NOT EXISTS (
      SELECT 1 FROM inbound_occurrence_purge_guards guard
      WHERE guard.source_library_id IS NOT DISTINCT FROM w.source_library_id
        AND ingest_occurrence_hash(guard.source_path)=ingest_occurrence_hash(w.source_path)
        AND guard.source_path COLLATE "C"=w.source_path COLLATE "C")
ORDER BY CASE WHEN state='failed' AND last_error_code='lease.lost' THEN 0 ELSE 1 END,
         CASE WHEN stage='inspection' THEN next_attempt_at END NULLS FIRST,
         CASE WHEN stage='inspection' THEN created_at ELSE updated_at END, id
LIMIT $5 FOR UPDATE SKIP LOCKED
