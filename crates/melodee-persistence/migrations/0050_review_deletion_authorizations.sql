-- Pre-catalog work has no Release or policy decision to attach an authorization to.
CREATE TABLE review_deletion_authorizations (
    work_item_id uuid PRIMARY KEY REFERENCES release_work_items(id) ON DELETE RESTRICT,
    actor_id uuid NOT NULL,
    actor_role text NOT NULL CHECK (actor_role IN ('administrator', 'editor')),
    note text,
    staging_path text CHECK (staging_path = 'releases/' || work_item_id::text || '/source'),
    canonical_path text,
    authorized_at timestamptz NOT NULL,
    completed_at timestamptz
);

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
        ((actor->>'authority' = 'administrator' OR (actor->>'authority' = 'editor' AND disposition = 'delete'))
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
        ((actor->>'authority' = 'administrator' OR (actor->>'authority' = 'editor' AND disposition = 'delete'))
         AND btrim(actor->>'actor') <> ''
         AND actor - ARRAY['authority', 'actor'] = '{}'::jsonb)
    )
);
