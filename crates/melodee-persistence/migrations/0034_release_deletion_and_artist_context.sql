-- ADR-0152 permits an explicitly confirmed administrator deletion to retain
-- catalog/audit tombstones after Melodee-owned media has been removed.
ALTER TABLE releases DROP CONSTRAINT releases_status_check;
ALTER TABLE releases ADD CONSTRAINT releases_status_check
    CHECK (status IN ('active', 'deprecated', 'deleted'));

-- ADR-0153 repairs only importer-created provisional Track identities whose
-- observation exactly matches the sole primary Release Artist in the same
-- Release. Raw observations and append-only decisions remain intact.
CREATE TEMP TABLE matching_release_artist_repairs ON COMMIT DROP AS
SELECT track_artist.artist_id AS source_artist_id,
       release_artist.artist_id AS target_artist_id,
       track_observed.id AS observed_artist_id,
       current_match.decision_id AS predecessor_decision_id,
       gen_random_uuid() AS decision_id,
       track_observed.release_id,
       track_observed.track_id
  FROM track_artists track_artist
  JOIN observed_artists track_observed
    ON track_observed.id = track_artist.observed_artist_id
  JOIN release_artists release_artist
    ON release_artist.release_id = track_observed.release_id
   AND release_artist.role = 'primary'
  JOIN observed_artists release_observed
    ON release_observed.id = release_artist.observed_artist_id
  JOIN artist_match_current current_match
    ON current_match.observed_artist_id = track_observed.id
  JOIN artist_match_decisions current_decision
    ON current_decision.id = current_match.decision_id
  JOIN artists source_artist ON source_artist.id = track_artist.artist_id
  JOIN artists target_artist ON target_artist.id = release_artist.artist_id
 WHERE track_artist.role = 'primary'
   AND track_observed.normalized_value = release_observed.normalized_value
   AND track_artist.artist_id <> release_artist.artist_id
   AND source_artist.status = 'provisional'
   AND target_artist.status IN ('provisional', 'verified')
   AND current_decision.artist_id = track_artist.artist_id
   AND current_decision.authority = 'automated'
   AND NOT current_decision.is_locked
   AND current_decision.reason = 'created a distinct provisional identity from manifest context'
   AND (SELECT count(*) FROM release_artists candidate
         WHERE candidate.release_id = track_observed.release_id
           AND candidate.role = 'primary') = 1
   AND (SELECT count(*) FROM track_artists candidate
         WHERE candidate.track_id = track_observed.track_id
           AND candidate.role = 'primary') = 1
   AND NOT EXISTS (
       SELECT 1 FROM release_artists other
        WHERE other.artist_id = track_artist.artist_id
   )
   AND NOT EXISTS (
       SELECT 1
         FROM track_artists other
         JOIN observed_artists other_observed ON other_observed.id = other.observed_artist_id
        WHERE other.artist_id = track_artist.artist_id
          AND (other_observed.release_id <> track_observed.release_id
               OR other_observed.normalized_value <> release_observed.normalized_value)
   );

CREATE UNIQUE INDEX matching_release_artist_repairs_source_idx
    ON matching_release_artist_repairs (source_artist_id);
CREATE UNIQUE INDEX matching_release_artist_repairs_observation_idx
    ON matching_release_artist_repairs (observed_artist_id);

INSERT INTO artist_match_decisions (
    id, observed_artist_id, artist_id, outcome, authority, confidence,
    reason, is_locked, predecessor_decision_id, payload, created_at
)
SELECT repair.decision_id, repair.observed_artist_id, repair.target_artist_id,
       'confirmed', 'automated', 9000,
       'resolved to the sole matching Release Artist from manifest context',
       false, repair.predecessor_decision_id,
       jsonb_build_object(
           'resolver_version', 'catalog.same-release-artist.v1',
           'migration_repair', true,
           'predecessor_decision_id', repair.predecessor_decision_id
       ),
       clock_timestamp()
  FROM matching_release_artist_repairs repair;

UPDATE artist_match_current current_match
   SET decision_id = repair.decision_id,
       updated_at = clock_timestamp()
  FROM matching_release_artist_repairs repair
 WHERE current_match.observed_artist_id = repair.observed_artist_id;

UPDATE track_artists track_artist
   SET artist_id = repair.target_artist_id
  FROM matching_release_artist_repairs repair
 WHERE track_artist.observed_artist_id = repair.observed_artist_id
   AND track_artist.artist_id = repair.source_artist_id;

UPDATE contributions contribution
   SET artist_id = repair.target_artist_id
  FROM matching_release_artist_repairs repair
 WHERE contribution.observed_artist_id = repair.observed_artist_id
   AND contribution.artist_id = repair.source_artist_id;

UPDATE artist_track_associations association
   SET artist_id = repair.target_artist_id
  FROM matching_release_artist_repairs repair
 WHERE association.artist_id = repair.source_artist_id;

UPDATE artists source_artist
   SET status = 'merged',
       merged_into_artist_id = repair.target_artist_id,
       revision = source_artist.revision + 1,
       updated_at = clock_timestamp()
  FROM matching_release_artist_repairs repair
 WHERE source_artist.id = repair.source_artist_id;

UPDATE artists target_artist
   SET revision = target_artist.revision + 1,
       updated_at = clock_timestamp()
 WHERE target_artist.id IN (
     SELECT DISTINCT target_artist_id FROM matching_release_artist_repairs
 );

INSERT INTO artist_merge_events (
    id, source_artist_id, target_artist_id, actor, reason,
    moved_relationships, created_at
)
SELECT gen_random_uuid(), repair.source_artist_id, repair.target_artist_id,
       'catalog.same-release-artist.v1',
       'Track observation exactly matched the sole primary Release Artist',
       jsonb_build_object(
           'release_artists', 0,
           'track_artists', 1,
           'contributions', 0,
           'aliases', 0,
           'external_identities', 0,
           'relationships', 0,
           'memberships', 0,
           'observations', 1,
           'release_id', repair.release_id,
           'track_id', repair.track_id
       ),
       clock_timestamp()
  FROM matching_release_artist_repairs repair;

INSERT INTO catalog_audit_events (
    event_id, entity_type, entity_id, event_type, actor, reason,
    payload, created_at
)
SELECT gen_random_uuid(), 'artist', repair.source_artist_id, 'artist_merged',
       'catalog.same-release-artist.v1',
       'Repaired importer-created same-Release Track Artist identity',
       jsonb_build_object(
           'target_artist_id', repair.target_artist_id,
           'release_id', repair.release_id,
           'track_id', repair.track_id
       ),
       clock_timestamp()
  FROM matching_release_artist_repairs repair;

DELETE FROM artist_catalog_aggregates aggregate
 WHERE aggregate.artist_id IN (
     SELECT source_artist_id FROM matching_release_artist_repairs
 );

INSERT INTO artist_catalog_aggregates (
    artist_id, release_count, billed_track_count, performance_track_count,
    all_credit_track_count, revision, updated_at
)
SELECT target.artist_id,
       count(DISTINCT track.release_id),
       count(DISTINCT association.track_id),
       count(DISTINCT association.track_id) FILTER (WHERE association.is_performance),
       count(DISTINCT association.track_id),
       1, clock_timestamp()
  FROM (
      SELECT DISTINCT target_artist_id AS artist_id
        FROM matching_release_artist_repairs
  ) target
  LEFT JOIN artist_track_associations association
    ON association.artist_id = target.artist_id
  LEFT JOIN tracks track ON track.id = association.track_id
 GROUP BY target.artist_id
ON CONFLICT (artist_id) DO UPDATE
    SET release_count = EXCLUDED.release_count,
        billed_track_count = EXCLUDED.billed_track_count,
        performance_track_count = EXCLUDED.performance_track_count,
        all_credit_track_count = EXCLUDED.all_credit_track_count,
        revision = artist_catalog_aggregates.revision + 1,
        updated_at = EXCLUDED.updated_at;

INSERT INTO catalog_projection_outbox (
    projection_name, entity_type, entity_id, operation, entity_revision,
    available_at, attempt_count, created_at
)
SELECT 'tantivy.catalog', 'artist', source_artist.id, 'delete',
       source_artist.revision, clock_timestamp(), 0, clock_timestamp()
  FROM artists source_artist
 WHERE source_artist.id IN (
     SELECT source_artist_id FROM matching_release_artist_repairs
 )
UNION ALL
SELECT 'tantivy.catalog', 'artist', target_artist.id, 'upsert',
       target_artist.revision, clock_timestamp(), 0, clock_timestamp()
  FROM artists target_artist
 WHERE target_artist.id IN (
     SELECT target_artist_id FROM matching_release_artist_repairs
 )
ON CONFLICT (projection_name, entity_type, entity_id, entity_revision)
DO NOTHING;
