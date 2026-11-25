WITH stages(stage, exits) AS (VALUES
    ('intake',ARRAY['ready']), ('inspection',ARRAY['normalizing']),
    ('normalization',ARRAY['validating']), ('validation',ARRAY['staged','needs_review']),
    ('publication',ARRAY['published']), ('catalog',ARRAY['cataloged']),
    ('policy',ARRAY['indexing','needs_review','quarantined','ignored']), ('indexing',ARRAY['indexed'])
), eligible AS (
    SELECT r.stage,r.id,r.source_path,
        coalesce(r.lease_expires_at > $1,false) AS running,
        greatest(r.stage_entered_at,r.discovered_at,r.next_attempt_at,
            CASE WHEN r.state='settling' THEN r.stable_since + make_interval(secs =>
                CASE WHEN s.ready_marker_seen THEN least($3::float8,$4::float8) ELSE $3 END) END) AS since
    FROM ingest_runnable r
    LEFT JOIN release_work_item_settling_snapshots s ON r.state='settling' AND s.id=r.latest_settling_snapshot_id
    WHERE (r.stage <> 'intake' OR r.state_reason IS DISTINCT FROM 'source-missing')
      AND (r.state <> 'failed' OR r.attempt_count < $2)
      AND (r.state <> 'settling' OR r.stable_since IS NOT NULL)
), waiting AS (
    SELECT *, count(*) FILTER (WHERE NOT running) OVER (PARTITION BY stage) AS count,
        count(*) FILTER (WHERE running) OVER (PARTITION BY stage) AS active,
        min(since) OVER (PARTITION BY stage) AS oldest_work,
        row_number() OVER (PARTITION BY stage ORDER BY running,since,id) AS rank
    FROM eligible WHERE since <= $1
)
SELECT s.stage,coalesce(w.count,0) AS waiting,coalesce(w.active,0) AS in_progress,
       CASE WHEN NOT w.running THEN w.source_path END AS oldest_release,
       CASE WHEN NOT w.running THEN w.id END AS oldest_item,
       CASE WHEN NOT w.running THEN w.since END AS oldest_since,c.latest AS last_completion,
       h.n AS completed_hour,
       coalesce(w.oldest_work <= $1 - interval '15 minutes'
           AND (c.latest IS NULL OR c.latest <= $1 - interval '15 minutes'),false) AS stalled
FROM stages s LEFT JOIN waiting w ON w.stage=s.stage AND w.rank=1
LEFT JOIN LATERAL (
    SELECT created_at AS latest FROM release_work_item_events
    WHERE to_state = ANY(s.exits) AND ingest_processing_stage(from_state)=s.stage
      AND from_state <> to_state AND created_at <= $1
    ORDER BY created_at DESC LIMIT 1
) c ON true
CROSS JOIN LATERAL (
    SELECT count(*) AS n FROM release_work_item_events
    WHERE to_state = ANY(s.exits) AND ingest_processing_stage(from_state)=s.stage
      AND from_state <> to_state AND created_at > $1 - interval '1 hour' AND created_at <= $1
) h
ORDER BY array_position(ARRAY['intake','inspection','normalization','validation','publication','catalog','policy','indexing'],s.stage)
