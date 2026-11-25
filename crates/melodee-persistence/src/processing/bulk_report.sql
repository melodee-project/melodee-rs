WITH b AS (
    SELECT e.source_path,coalesce(w.discovered_at,p.discovered_at) AS discovered_at,
           coalesce(w.first_processing_outcome,p.outcome) AS outcome,
           coalesce(w.first_processing_outcome_at,p.finished_at) AS finished
    FROM ingest_bulk_entries e LEFT JOIN release_work_items w ON w.id=e.work_item_id
    LEFT JOIN ingest_purged_arrivals p ON p.id=e.work_item_id
    WHERE batch_id=$1
), bounds AS (
    SELECT min(discovered_at) AS started,
           CASE WHEN count(finished)=count(*) THEN max(finished) END AS ended FROM b
), population AS (
    SELECT 'bulk' AS lane,b.* FROM b UNION ALL
    SELECT 'ordinary',w.source_path,w.discovered_at,w.first_processing_outcome,w.first_processing_outcome_at
    FROM release_work_items w CROSS JOIN bounds t
    WHERE w.bulk_submission_id IS NULL AND w.discovered_at BETWEEN t.started AND coalesce(t.ended,now())
    UNION ALL
    SELECT 'ordinary',NULL,p.discovered_at,p.outcome,p.finished_at
    FROM ingest_purged_arrivals p CROSS JOIN bounds t WHERE p.bulk_submission_id IS NULL
        AND p.discovered_at BETWEEN t.started AND coalesce(t.ended,now())
), durations AS (
    SELECT *, extract(epoch FROM finished-discovered_at) AS seconds FROM population
), stats AS (
    SELECT l.lane, count(d.lane) AS total, count(d.discovered_at) AS arrived,
           count(*) FILTER (WHERE outcome='playable') AS playable,
           count(*) FILTER (WHERE outcome='failed') AS failed,
           count(*) FILTER (WHERE outcome='held') AS manual_holds,
           count(*) FILTER (WHERE outcome='excluded') AS ignored_or_deleted,
           count(d.lane) FILTER (WHERE outcome IS NULL) AS pending_including_unseen,
           count(*) FILTER (WHERE outcome='playable' AND seconds<=1800) AS playable_within_30_minutes,
           round(100.0 * count(*) FILTER (WHERE outcome='playable' AND seconds<=1800)
                 / nullif(count(d.lane),0),2) AS percent_of_all_playable_within_30_minutes,
           (array_agg(source_path ORDER BY discovered_at,source_path)
              FILTER (WHERE discovered_at IS NOT NULL AND outcome IS NULL))[1] AS oldest_pending_release,
           extract(epoch FROM now()-min(discovered_at) FILTER (WHERE outcome IS NULL)) AS oldest_pending_seconds,
           round(avg(seconds) FILTER (WHERE outcome='playable'),1) AS mean_playable_seconds,
           percentile_disc(0.95) WITHIN GROUP (ORDER BY seconds) FILTER (WHERE outcome='playable') AS p95_playable_seconds,
           max(seconds) FILTER (WHERE outcome='playable') AS slowest_playable_seconds,
           (array_agg(source_path ORDER BY seconds DESC,source_path) FILTER (WHERE outcome='playable'))[1] AS slowest_playable_release
    FROM (VALUES ('bulk'),('ordinary')) l(lane) LEFT JOIN durations d ON d.lane=l.lane GROUP BY l.lane
)
SELECT jsonb_build_object('submission',s.id,'registered_at',s.registered_at,'first_arrival',t.started,
    'automatic_processing_finished',t.ended,'elapsed_seconds',extract(epoch FROM coalesce(t.ended,now())-t.started),
    'bulk_playable_within_10_hours',(SELECT count(*) FROM b WHERE outcome='playable'
        AND finished <= t.started + interval '10 hours'),
    'bulk_playable_per_hour',round((SELECT count(*) FROM b WHERE outcome='playable') * 3600.0 /
        nullif(extract(epoch FROM coalesce(t.ended,now()) - t.started),0),1),
    'observed_at',now(),'cohorts',(SELECT jsonb_object_agg(lane,to_jsonb(stats)-'lane') FROM stats))
FROM ingest_bulk_submissions s CROSS JOIN bounds t WHERE s.id=$1
