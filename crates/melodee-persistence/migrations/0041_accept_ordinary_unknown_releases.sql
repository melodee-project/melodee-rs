-- ADR-0177 corrects the obsolete built-in v1 rule that routed every ordinary
-- Unknown-classification Release to NeedsReview. Built-in rule-set v2 omits
-- that catch-all, so ordinary Releases fall through to Accept. Explicitly
-- authored UnknownClassification rules remain supported.
--
-- Existing decisions are durable evidence and are not deleted or rewritten
-- without attribution. This migration targets only the exact system-created
-- v1 POL-UNKNOWN pending decision shape, records a SystemUpgrade actor in the
-- append-only audit, and routes those Releases through the same non-destructive
-- Accept boundary used by the policy store. Operator-authored overrides and
-- every other rule/disposition are outside the target predicate.
--
-- Forward plan: deploy this migration with the v2 built-in rule set and set
-- MELODEE__POLICY__RULE_SET_VERSION=2 (or a later operator-authored version).
-- A binary downgrade retains the reconciled audit and active Releases; it must
-- continue using rule-set v2 or later. There is no destructive down migration.

ALTER TABLE policy_decisions DROP CONSTRAINT policy_decisions_actor_check;
ALTER TABLE policy_decisions ADD CONSTRAINT policy_decisions_actor_check CHECK (
    jsonb_typeof(actor) = 'object' AND (
        (actor->>'authority' = 'system'
         AND actor->>'policy_rule_set_version' = rule_set_version::text
         AND actor - ARRAY['authority', 'policy_rule_set_version'] = '{}'::jsonb)
        OR
        (actor->>'authority' = 'system_upgrade'
         AND btrim(actor->>'migration') <> ''
         AND actor->>'policy_rule_set_version' = rule_set_version::text
         AND actor - ARRAY[
             'authority', 'migration', 'policy_rule_set_version'
         ] = '{}'::jsonb)
        OR
        (actor->>'authority' = 'administrator'
         AND btrim(actor->>'actor') <> ''
         AND actor - ARRAY['authority', 'actor'] = '{}'::jsonb)
    )
);

ALTER TABLE policy_decision_audit DROP CONSTRAINT policy_decision_audit_actor_check;
ALTER TABLE policy_decision_audit ADD CONSTRAINT policy_decision_audit_actor_check CHECK (
    jsonb_typeof(actor) = 'object' AND (
        (actor->>'authority' = 'system'
         AND actor->>'policy_rule_set_version' = rule_set_version::text
         AND actor - ARRAY['authority', 'policy_rule_set_version'] = '{}'::jsonb)
        OR
        (actor->>'authority' = 'system_upgrade'
         AND btrim(actor->>'migration') <> ''
         AND actor->>'policy_rule_set_version' = rule_set_version::text
         AND actor - ARRAY[
             'authority', 'migration', 'policy_rule_set_version'
         ] = '{}'::jsonb)
        OR
        (actor->>'authority' = 'administrator'
         AND btrim(actor->>'actor') <> ''
         AND actor - ARRAY['authority', 'actor'] = '{}'::jsonb)
    )
);

CREATE TEMPORARY TABLE policy_default_unknown_v1_upgrade
ON COMMIT DROP AS
SELECT decision.id AS decision_id,
       decision.release_id,
       decision.work_item_id,
       item.correlation_id,
       clock_timestamp() AS upgraded_at
  FROM policy_decisions decision
  JOIN policy_rule_sets rule_set
    ON rule_set.id = decision.rule_set_id
   AND rule_set.version = decision.rule_set_version
  JOIN catalog_imports import
    ON import.release_id = decision.release_id
   AND import.work_item_id = decision.work_item_id
  JOIN release_work_items item ON item.id = decision.work_item_id
  JOIN releases release ON release.id = decision.release_id
 WHERE decision.rule_set_id = '01989f00-7000-7000-8000-000500000001'::uuid
   AND decision.rule_set_version = 1
   AND rule_set.name = 'phase5-default-rules'
   AND rule_set.created_by = '{
       "authority": "system", "policy_rule_set_version": 1
   }'::jsonb
   AND decision.matched_rule_ids = '["POL-UNKNOWN"]'::jsonb
   AND decision.disposition = 'review'
   AND decision.evidence_summary = 'Release classification is Unknown'
   AND decision.actor = '{
       "authority": "system", "policy_rule_set_version": 1
   }'::jsonb
   AND decision.completion_state = 'pending'
   AND decision.completed_at IS NULL
   AND decision.override_reason IS NULL
   AND item.state = 'needs_review'
   AND item.lease_owner IS NULL
   AND item.lease_acquired_at IS NULL
   AND item.lease_expires_at IS NULL
   AND release.status = 'deprecated';

UPDATE policy_decisions decision
   SET matched_rule_ids = '[]'::jsonb,
       disposition = 'accept',
       evidence_summary = 'No configured policy rule matched; accepting Release',
       actor = jsonb_build_object(
           'authority', 'system_upgrade',
           'migration', '0041_accept_ordinary_unknown_releases',
           'policy_rule_set_version', decision.rule_set_version
       ),
       decided_at = target.upgraded_at,
       completion_state = 'completed',
       completed_at = target.upgraded_at,
       override_reason = 'Built-in v1 POL-UNKNOWN review was removed by ADR-0177'
  FROM policy_default_unknown_v1_upgrade target
 WHERE decision.id = target.decision_id;

UPDATE releases release
   SET status = 'active',
       revision = revision + 1,
       updated_at = target.upgraded_at
  FROM policy_default_unknown_v1_upgrade target
 WHERE release.id = target.release_id
   AND release.status = 'deprecated';

UPDATE release_work_items item
   SET state = 'indexing',
       state_reason = 'policy.disposition.accept',
       next_attempt_at = NULL,
       completed_at = NULL,
       updated_at = target.upgraded_at
  FROM policy_default_unknown_v1_upgrade target
 WHERE item.id = target.work_item_id
   AND item.state = 'needs_review';

INSERT INTO release_work_item_events (
    work_item_id, event_type, from_state, to_state, reason_code,
    worker_id, correlation_id, created_at, metadata
)
SELECT target.work_item_id,
       'transitioned',
       'needs_review',
       'indexing',
       'policy.default_unknown_v2_reconciled',
       NULL,
       target.correlation_id,
       target.upgraded_at,
       jsonb_build_object(
           'decision_id', target.decision_id,
           'disposition', 'accept',
           'migration', '0041_accept_ordinary_unknown_releases'
       )
  FROM policy_default_unknown_v1_upgrade target;
