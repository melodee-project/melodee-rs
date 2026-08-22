-- Release classification, Track novelty, versioned policy rules,
-- durable decisions, and dry-run/test results.
--
-- This migration is additive. No statement reads, moves, rewrites, or deletes
-- media bytes or canonical storage paths. Classification and novelty are
-- pure observations stored as durable evidence. Policy decisions are recorded
-- before any destructive action; actual file deletion is a separate boundary.
-- Forward/rollback plan: this migration is additive to catalog/media facts. A
-- forward repair may rebuild reproducible classification/novelty projections
-- from immutable catalog imports, but decision audit and operator/catalog
-- assertions are durable evidence and are never discarded in place. A binary
-- downgrade retains the schema. There is no destructive down
-- migration; a full schema rollback drains workers and restores a verified
-- pre-0008 backup into a new database, then explicitly accounts for evidence
-- created after that backup. Catalog, publication, identity, and
-- media rows are never rewritten by a rollback script.

-- Treating Cataloged as an unleased completion boundary claims that same
-- state for policy evaluation (SPEC 11.2 has no PolicyEvaluating state), so
-- Cataloged may now be either wholly leased or wholly unleased.
ALTER TABLE release_work_items
    DROP CONSTRAINT release_work_items_catalog_lease_check;
ALTER TABLE release_work_items
    ADD CONSTRAINT release_work_items_catalog_lease_check CHECK (
        (state <> 'cataloged'
            OR (lease_owner IS NULL AND lease_acquired_at IS NULL AND lease_expires_at IS NULL)
            OR (lease_owner IS NOT NULL
                AND lease_acquired_at IS NOT NULL AND lease_expires_at IS NOT NULL))
        AND (state <> 'cataloging'
            OR (lease_owner IS NOT NULL
                AND lease_acquired_at IS NOT NULL AND lease_expires_at IS NOT NULL)
            OR (lease_owner IS NULL
                AND lease_acquired_at IS NULL AND lease_expires_at IS NULL))
    );

ALTER TABLE release_work_items
    DROP CONSTRAINT release_work_items_retry_consistency_check;
ALTER TABLE release_work_items
    ADD CONSTRAINT release_work_items_retry_consistency_check CHECK (
        next_attempt_at IS NULL OR state IN ('failed', 'cataloging', 'cataloged')
    );

CREATE INDEX release_work_items_policy_claim_idx
    ON release_work_items (next_attempt_at NULLS FIRST, updated_at, id)
    WHERE state = 'cataloged';

-- Explicit positive evidence that two AudioIdentities are distinct recordings.
-- A generic conflict/match row is not enough to infer this policy fact.
ALTER TABLE audio_identity_evidence
    ADD CONSTRAINT audio_identity_evidence_exact_uniq UNIQUE (
        id, source_audio_identity_id, candidate_audio_identity_id,
        polarity, evidence_kind
    );

CREATE TABLE audio_identity_alternate_recordings (
    source_audio_identity_id uuid NOT NULL,
    alternate_audio_identity_id uuid NOT NULL,
    evidence_id uuid NOT NULL UNIQUE,
    evidence_polarity text GENERATED ALWAYS AS ('supports'::text) STORED,
    evidence_kind text GENERATED ALWAYS AS ('manual'::text) STORED,
    actor text NOT NULL,
    reason text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (source_audio_identity_id, alternate_audio_identity_id),
    CONSTRAINT audio_identity_alternate_recordings_distinct_check CHECK (
        source_audio_identity_id <> alternate_audio_identity_id
    ),
    CONSTRAINT audio_identity_alternate_recordings_reason_check CHECK (
        btrim(actor) <> '' AND btrim(reason) <> ''
    ),
    CONSTRAINT audio_identity_alternate_recordings_evidence_fk FOREIGN KEY (
        evidence_id, source_audio_identity_id, alternate_audio_identity_id,
        evidence_polarity, evidence_kind
    ) REFERENCES audio_identity_evidence (
        id, source_audio_identity_id, candidate_audio_identity_id,
        polarity, evidence_kind
    ) ON DELETE RESTRICT
);

CREATE INDEX audio_identity_alternate_recordings_reverse_idx
    ON audio_identity_alternate_recordings (
        alternate_audio_identity_id, source_audio_identity_id
    );
CREATE UNIQUE INDEX audio_identity_alternate_recordings_unordered_uniq
    ON audio_identity_alternate_recordings (
        LEAST(source_audio_identity_id, alternate_audio_identity_id),
        GREATEST(source_audio_identity_id, alternate_audio_identity_id)
    );

-- Explicit, audited fact that resolver/operator work found no matching
-- catalog identity. Pairwise support evidence is intentionally insufficient.
CREATE TABLE audio_identity_catalog_assertions (
    id uuid PRIMARY KEY,
    audio_identity_id uuid NOT NULL UNIQUE
        REFERENCES audio_identities (id) ON DELETE RESTRICT,
    outcome text NOT NULL,
    actor text NOT NULL,
    reason text NOT NULL,
    evidence jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT audio_identity_catalog_assertions_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT audio_identity_catalog_assertions_outcome_check CHECK (
        outcome = 'no_catalog_match'
    ),
    CONSTRAINT audio_identity_catalog_assertions_reason_check CHECK (
        btrim(actor) <> '' AND btrim(reason) <> ''
    ),
    CONSTRAINT audio_identity_catalog_assertions_evidence_check CHECK (
        jsonb_typeof(evidence) = 'object'
    )
);

-- Independent catalog-analysis knowledge used by the CatalogContext
-- classification tier. This is deliberately separate from the classification
-- being validated so provenance cannot become circular.
CREATE TABLE release_catalog_trait_assertions (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    trait text NOT NULL,
    source_version text NOT NULL,
    actor text NOT NULL,
    reason text NOT NULL,
    evidence jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT release_catalog_trait_assertions_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT release_catalog_trait_assertions_trait_check CHECK (trait IN (
        'studio', 'ep', 'single', 'live', 'compilation', 'greatest_hits',
        'collection', 'box_set', 'bootleg', 'demo', 'promo', 'broadcast',
        'radio_show', 'dj_mix', 'remix', 'tribute', 'soundtrack',
        'cast_recording', 'spoken_word', 'comedy', 'audiobook', 'reissue',
        'remaster', 'other', 'unknown'
    )),
    CONSTRAINT release_catalog_trait_assertions_provenance_check CHECK (
        btrim(source_version) <> '' AND btrim(actor) <> '' AND btrim(reason) <> ''
        AND jsonb_typeof(evidence) = 'object'
    ),
    UNIQUE (release_id, trait)
);

CREATE INDEX release_catalog_trait_assertions_release_idx
    ON release_catalog_trait_assertions (release_id, trait);

-- =========================================================================
-- Release classification: multi-valued traits with explainable evidence
-- =========================================================================

CREATE TABLE release_classifications (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL UNIQUE REFERENCES releases (id) ON DELETE RESTRICT,
    confidence integer NOT NULL CHECK (confidence BETWEEN 0 AND 10000),
    classifier_version text NOT NULL,
    classified_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT release_classifications_classifier_check CHECK (
        btrim(classifier_version) <> ''
    )
);

-- Multi-valued traits: a Release may have any combination of traits.
CREATE TABLE release_traits (
    classification_id uuid NOT NULL REFERENCES release_classifications (id) ON DELETE CASCADE,
    trait text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (classification_id, trait),
    CONSTRAINT release_traits_check CHECK (trait IN (
        'studio', 'ep', 'single', 'live', 'compilation', 'greatest_hits',
        'collection', 'box_set', 'bootleg', 'demo', 'promo', 'broadcast',
        'radio_show', 'dj_mix', 'remix', 'tribute', 'soundtrack',
        'cast_recording', 'spoken_word', 'comedy', 'audiobook', 'reissue',
        'remaster', 'other', 'unknown'
    ))
);

CREATE INDEX release_traits_release_idx
    ON release_traits (classification_id) INCLUDE (trait);

-- Explainable evidence for each trait (PRD-CLASS-002, PRD-CLASS-003).
CREATE TABLE release_classification_evidence (
    id uuid NOT NULL REFERENCES release_classifications (id) ON DELETE CASCADE,
    trait text NOT NULL,
    source text NOT NULL,
    kind text NOT NULL,
    confidence integer NOT NULL CHECK (confidence BETWEEN 0 AND 10000),
    detail text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (id, trait, source, kind, detail)
    ,CONSTRAINT release_classification_evidence_trait_check CHECK (trait IN (
        'studio', 'ep', 'single', 'live', 'compilation', 'greatest_hits',
        'collection', 'box_set', 'bootleg', 'demo', 'promo', 'broadcast',
        'radio_show', 'dj_mix', 'remix', 'tribute', 'soundtrack',
        'cast_recording', 'spoken_word', 'comedy', 'audiobook', 'reissue',
        'remaster', 'other', 'unknown'
    ))
    ,CONSTRAINT release_classification_evidence_source_check CHECK (source IN (
        'administrator', 'embedded_external_id', 'structured_tags',
        'catalog_context', 'release_title', 'directory_name',
        'parent_directory_name'
    ))
    ,CONSTRAINT release_classification_evidence_kind_check CHECK (kind IN (
        'title_pattern', 'structured_field', 'embedded_external_id',
        'catalog_known_release', 'catalog_artist_match', 'directory_name',
        'parent_directory_name', 'manual_override'
    ))
    ,CONSTRAINT release_classification_evidence_detail_check CHECK (btrim(detail) <> '')
);

ALTER TABLE release_classification_evidence
    ADD CONSTRAINT release_classification_evidence_trait_fk FOREIGN KEY (id, trait)
        REFERENCES release_traits (classification_id, trait) ON DELETE RESTRICT;

-- Evidence source trust-rank index: admin > embedded > tags > catalog > title > dir.
CREATE INDEX release_classification_evidence_source_idx
    ON release_classification_evidence (id, source);

-- Constraint: directory/path evidence must never be the strongest source
-- for a classification that could authorize destructive action.
-- This is enforced at the policy layer (evidence_source trust ranking),
-- but we record the source for audit.
ALTER TABLE release_classifications
    ADD CONSTRAINT release_classifications_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    );

-- =========================================================================
-- Track novelty analysis
-- =========================================================================

CREATE TABLE track_novelty (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    media_asset_id uuid NULL REFERENCES media_assets (id) ON DELETE RESTRICT,
    category text NOT NULL,
    confidence integer NOT NULL CHECK (confidence BETWEEN 0 AND 10000),
    evidence_kind text NOT NULL,
    detail text NOT NULL,
    evaluated_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT track_novelty_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT track_novelty_category_check CHECK (
        category IN (
            'exact_duplicate', 'known_audio_identity', 'alternate_recording',
            'new_track', 'uncertain'
        )
    ),
    CONSTRAINT track_novelty_evidence_kind_check CHECK (btrim(evidence_kind) <> ''),
    CONSTRAINT track_novelty_category_evidence_check CHECK (
        (category = 'exact_duplicate' AND evidence_kind = 'byte_hash_match')
        OR (category = 'known_audio_identity' AND evidence_kind = 'audio_identity_match')
        OR (category = 'alternate_recording' AND evidence_kind = 'audio_identity_distinct')
        OR (category = 'new_track' AND evidence_kind = 'no_catalog_match')
        OR (category = 'uncertain' AND evidence_kind = 'insufficient_evidence')
    ),
    CONSTRAINT track_novelty_detail_check CHECK (btrim(detail) <> ''),
 -- A Track appears at most once in a release's novelty evaluation
    UNIQUE (release_id, track_id)
);

CREATE INDEX track_novelty_release_idx ON track_novelty (release_id, category);
CREATE INDEX track_novelty_audio_identity_idx ON track_novelty (audio_identity_id, category);

-- Aggregate novelty evaluation per Release (PRD-POLICY-005).
CREATE TABLE release_novelty_evaluations (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL UNIQUE REFERENCES releases (id) ON DELETE RESTRICT,
    policy_rule_set_id uuid NOT NULL,
    exact_duplicate_count integer NOT NULL CHECK (exact_duplicate_count >= 0),
    known_audio_identity_count integer NOT NULL CHECK (known_audio_identity_count >= 0),
    alternate_recording_count integer NOT NULL CHECK (alternate_recording_count >= 0),
    new_track_count integer NOT NULL CHECK (new_track_count >= 0),
    uncertain_count integer NOT NULL CHECK (uncertain_count >= 0),
    total_track_count integer NOT NULL CHECK (total_track_count > 0),
    evaluated_at timestamptz NOT NULL,
    evaluator_version text NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT release_novelty_evaluations_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT release_novelty_counts_check CHECK (
        exact_duplicate_count + known_audio_identity_count
        + alternate_recording_count + new_track_count + uncertain_count = total_track_count
    ),
    CONSTRAINT release_novelty_version_check CHECK (btrim(evaluator_version) <> '')
);

CREATE INDEX release_novelty_release_idx
    ON release_novelty_evaluations (release_id);

ALTER TABLE release_novelty_evaluations
    ADD CONSTRAINT release_novelty_evaluations_id_release_uniq UNIQUE (id, release_id);

ALTER TABLE tracks
    ADD CONSTRAINT tracks_policy_fact_uniq UNIQUE (
        id, release_id, audio_identity_id, playable_media_asset_id
    );

ALTER TABLE track_novelty
    ADD COLUMN evaluation_id uuid NOT NULL,
    ALTER COLUMN media_asset_id SET NOT NULL,
    ADD CONSTRAINT track_novelty_evaluation_fk FOREIGN KEY (evaluation_id, release_id)
        REFERENCES release_novelty_evaluations (id, release_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT track_novelty_exact_track_fk FOREIGN KEY (
        track_id, release_id, audio_identity_id, media_asset_id
    ) REFERENCES tracks (
        id, release_id, audio_identity_id, playable_media_asset_id
    ) ON DELETE RESTRICT;

CREATE INDEX track_novelty_evaluation_idx ON track_novelty (evaluation_id, track_id);

-- Detail rows are inserted set-wise first under the deferred FK. The single
-- aggregate INSERT then proves its counts exactly match immutable detail.
CREATE FUNCTION validate_release_novelty_aggregate() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    stored_total bigint;
    stored_exact bigint;
    stored_known bigint;
    stored_alternate bigint;
    stored_new bigint;
    stored_uncertain bigint;
BEGIN
    SELECT count(*),
           count(*) FILTER (WHERE category = 'exact_duplicate'),
           count(*) FILTER (WHERE category = 'known_audio_identity'),
           count(*) FILTER (WHERE category = 'alternate_recording'),
           count(*) FILTER (WHERE category = 'new_track'),
           count(*) FILTER (WHERE category = 'uncertain')
      INTO stored_total, stored_exact, stored_known, stored_alternate,
           stored_new, stored_uncertain
      FROM track_novelty
     WHERE evaluation_id = NEW.id AND release_id = NEW.release_id;

    IF stored_total <> NEW.total_track_count
       OR stored_exact <> NEW.exact_duplicate_count
       OR stored_known <> NEW.known_audio_identity_count
       OR stored_alternate <> NEW.alternate_recording_count
       OR stored_new <> NEW.new_track_count
       OR stored_uncertain <> NEW.uncertain_count THEN
        RAISE EXCEPTION 'novelty aggregate does not match durable Track evidence';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER release_novelty_aggregate_integrity
AFTER INSERT ON release_novelty_evaluations
FOR EACH ROW EXECUTE FUNCTION validate_release_novelty_aggregate();

CREATE FUNCTION reject_policy_fact_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'classification and novelty facts are immutable';
END;
$$;

CREATE TRIGGER release_classifications_immutable
BEFORE UPDATE OR DELETE ON release_classifications
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();
CREATE TRIGGER release_traits_immutable
BEFORE UPDATE OR DELETE ON release_traits
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();
CREATE TRIGGER release_classification_evidence_immutable
BEFORE UPDATE OR DELETE ON release_classification_evidence
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();
CREATE TRIGGER release_novelty_evaluations_immutable
BEFORE UPDATE OR DELETE ON release_novelty_evaluations
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();
CREATE TRIGGER track_novelty_immutable
BEFORE UPDATE OR DELETE ON track_novelty
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();
CREATE TRIGGER audio_identity_alternate_recordings_immutable
BEFORE UPDATE OR DELETE ON audio_identity_alternate_recordings
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();
CREATE TRIGGER audio_identity_catalog_assertions_immutable
BEFORE UPDATE OR DELETE ON audio_identity_catalog_assertions
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();
CREATE TRIGGER release_catalog_trait_assertions_immutable
BEFORE UPDATE OR DELETE ON release_catalog_trait_assertions
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();

-- =========================================================================
-- Versioned policy rule sets and durable decisions
-- =========================================================================

CREATE TABLE policy_rule_sets (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    version integer NOT NULL,
    rules jsonb NOT NULL,
    created_by jsonb NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT policy_rule_sets_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT policy_rule_sets_version_check CHECK (version > 0),
    CONSTRAINT policy_rule_sets_name_check CHECK (btrim(name) <> ''),
    CONSTRAINT policy_rule_sets_rules_check CHECK (jsonb_typeof(rules) = 'array'),
    CONSTRAINT policy_rule_sets_actor_check CHECK (
        jsonb_typeof(created_by) = 'object' AND (
            (created_by->>'authority' = 'system'
             AND created_by->>'policy_rule_set_version' = version::text
             AND created_by - ARRAY['authority', 'policy_rule_set_version'] = '{}'::jsonb)
            OR
            (created_by->>'authority' = 'administrator'
             AND btrim(created_by->>'actor') <> ''
             AND created_by - ARRAY['authority', 'actor'] = '{}'::jsonb)
        )
    ),
    UNIQUE (name, version),
    UNIQUE (id, version)
);

CREATE INDEX policy_rule_sets_name_idx ON policy_rule_sets (name, version DESC);

CREATE FUNCTION reject_policy_rule_set_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'policy rule sets are immutable; create a new version';
END;
$$;

CREATE TRIGGER policy_rule_sets_append_only
BEFORE UPDATE OR DELETE ON policy_rule_sets
FOR EACH ROW EXECUTE FUNCTION reject_policy_rule_set_rewrite();

ALTER TABLE release_novelty_evaluations
    ADD CONSTRAINT release_novelty_evaluations_rule_fk FOREIGN KEY (policy_rule_set_id)
        REFERENCES policy_rule_sets (id) ON DELETE RESTRICT,
    ADD CONSTRAINT release_novelty_evaluations_decision_uniq UNIQUE (
        id, release_id, policy_rule_set_id
    );

-- Classification is reproducible against the exact immutable rule-set
-- version used by the evaluation that created it; standalone unversioned
-- classification is not authoritative persistence.
ALTER TABLE release_classifications
    ADD COLUMN rule_set_id uuid NOT NULL,
    ADD COLUMN rule_set_version integer NOT NULL,
    ADD CONSTRAINT release_classifications_rule_fk FOREIGN KEY (
        rule_set_id, rule_set_version
    ) REFERENCES policy_rule_sets (id, version) ON DELETE RESTRICT,
    ADD CONSTRAINT release_classifications_decision_uniq UNIQUE (
        id, release_id, rule_set_id, rule_set_version
    );

ALTER TABLE catalog_imports
    ADD CONSTRAINT catalog_imports_item_release_policy_uniq UNIQUE (work_item_id, release_id);

-- Durable decision record that MUST exist before any destructive action
-- (PRD-POLICY-007, SPEC section 15).
CREATE TABLE policy_decisions (
    id uuid PRIMARY KEY,
    classification_id uuid NOT NULL,
    novelty_evaluation_id uuid NOT NULL,
    release_id uuid NOT NULL UNIQUE REFERENCES releases (id) ON DELETE RESTRICT,
    work_item_id uuid NOT NULL,
    rule_set_id uuid NOT NULL,
    rule_set_version integer NOT NULL,
    matched_rule_ids jsonb NOT NULL,
    disposition text NOT NULL,
    evidence_summary text NOT NULL,
    source_fingerprint text NOT NULL,
    actor jsonb NOT NULL,
    decided_at timestamptz NOT NULL,
    completion_state text NOT NULL DEFAULT 'pending',
    completed_at timestamptz NULL,
    override_reason text NULL,

    CONSTRAINT policy_decisions_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT policy_decisions_disposition_check CHECK (
        disposition IN ('accept', 'review', 'ignore', 'quarantine', 'delete')
    ),
    CONSTRAINT policy_decisions_completion_check CHECK (
        completion_state IN ('pending', 'completed', 'failed')
    ),
    CONSTRAINT policy_decisions_completion_time_check CHECK (
        (completion_state = 'pending' AND completed_at IS NULL)
        OR (completion_state IN ('completed', 'failed')
            AND completed_at IS NOT NULL AND completed_at >= decided_at)
    ),
    CONSTRAINT policy_decisions_version_check CHECK (rule_set_version > 0),
    CONSTRAINT policy_decisions_summary_check CHECK (btrim(evidence_summary) <> ''),
    CONSTRAINT policy_decisions_rule_ids_check CHECK (jsonb_typeof(matched_rule_ids) = 'array'),
    CONSTRAINT policy_decisions_actor_check CHECK (
        jsonb_typeof(actor) = 'object' AND (
            (actor->>'authority' = 'system'
             AND actor->>'policy_rule_set_version' = rule_set_version::text
             AND actor - ARRAY['authority', 'policy_rule_set_version'] = '{}'::jsonb)
            OR
            (actor->>'authority' = 'administrator'
             AND btrim(actor->>'actor') <> ''
             AND actor - ARRAY['authority', 'actor'] = '{}'::jsonb)
        )
    ),
    CONSTRAINT policy_decisions_fingerprint_check CHECK (
        source_fingerprint ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT policy_decisions_override_reason_check CHECK (
        override_reason IS NULL OR btrim(override_reason) <> ''
    ),
    CONSTRAINT policy_decisions_rule_fk FOREIGN KEY (rule_set_id, rule_set_version)
        REFERENCES policy_rule_sets (id, version) ON DELETE RESTRICT,
    CONSTRAINT policy_decisions_import_fk FOREIGN KEY (work_item_id, release_id)
        REFERENCES catalog_imports (work_item_id, release_id) ON DELETE RESTRICT,
    CONSTRAINT policy_decisions_classification_fk FOREIGN KEY (
        classification_id, release_id, rule_set_id, rule_set_version
    ) REFERENCES release_classifications (
        id, release_id, rule_set_id, rule_set_version
    ) ON DELETE RESTRICT,
    CONSTRAINT policy_decisions_novelty_fk FOREIGN KEY (
        novelty_evaluation_id, release_id, rule_set_id
    ) REFERENCES release_novelty_evaluations (
        id, release_id, policy_rule_set_id
    ) ON DELETE RESTRICT
);

CREATE INDEX policy_decisions_release_idx ON policy_decisions (release_id);
CREATE INDEX policy_decisions_rule_set_idx ON policy_decisions (rule_set_id, rule_set_version);
CREATE INDEX policy_decisions_actor_idx ON policy_decisions (
    (actor->>'authority'),
    (actor->>'actor')
);

-- Append-only audit for every policy decision change.
CREATE TABLE policy_decision_audit (
    id uuid NOT NULL,
    classification_id uuid NOT NULL,
    novelty_evaluation_id uuid NOT NULL,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    work_item_id uuid NOT NULL,
    rule_set_id uuid NOT NULL,
    rule_set_version integer NOT NULL,
    matched_rule_ids jsonb NOT NULL,
    disposition text NOT NULL,
    evidence_summary text NOT NULL,
    source_fingerprint text NOT NULL,
    actor jsonb NOT NULL,
    decided_at timestamptz NOT NULL,
    completion_state text NOT NULL,
    completed_at timestamptz NULL,
    override_reason text NULL,
    event text NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT policy_decision_audit_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT policy_decision_audit_disposition_check CHECK (
        disposition IN ('accept', 'review', 'ignore', 'quarantine', 'delete')
    ),
    CONSTRAINT policy_decision_audit_completion_check CHECK (
        completion_state IN ('pending', 'completed', 'failed')
    ),
    CONSTRAINT policy_decision_audit_completion_time_check CHECK (
        (completion_state = 'pending' AND completed_at IS NULL)
        OR (completion_state IN ('completed', 'failed')
            AND completed_at IS NOT NULL AND completed_at >= decided_at)
    ),
    CONSTRAINT policy_decision_audit_version_check CHECK (rule_set_version > 0),
    CONSTRAINT policy_decision_audit_summary_check CHECK (btrim(evidence_summary) <> ''),
    CONSTRAINT policy_decision_audit_rule_ids_check CHECK (
        jsonb_typeof(matched_rule_ids) = 'array'
    ),
    CONSTRAINT policy_decision_audit_override_reason_check CHECK (
        override_reason IS NULL OR btrim(override_reason) <> ''
    ),
    CONSTRAINT policy_decision_audit_event_check CHECK (
        event IN ('created', 'updated', 'overridden', 'completed', 'failed')
    ),
    CONSTRAINT policy_decision_audit_fingerprint_check CHECK (
        source_fingerprint ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT policy_decision_audit_actor_check CHECK (
        jsonb_typeof(actor) = 'object' AND (
            (actor->>'authority' = 'system'
             AND actor->>'policy_rule_set_version' = rule_set_version::text
             AND actor - ARRAY['authority', 'policy_rule_set_version'] = '{}'::jsonb)
            OR
            (actor->>'authority' = 'administrator'
             AND btrim(actor->>'actor') <> ''
             AND actor - ARRAY['authority', 'actor'] = '{}'::jsonb)
        )
    ),
    CONSTRAINT policy_decision_audit_rule_fk FOREIGN KEY (rule_set_id, rule_set_version)
        REFERENCES policy_rule_sets (id, version) ON DELETE RESTRICT,
    CONSTRAINT policy_decision_audit_import_fk FOREIGN KEY (work_item_id, release_id)
        REFERENCES catalog_imports (work_item_id, release_id) ON DELETE RESTRICT,
    CONSTRAINT policy_decision_audit_classification_fk FOREIGN KEY (
        classification_id, release_id, rule_set_id, rule_set_version
    ) REFERENCES release_classifications (
        id, release_id, rule_set_id, rule_set_version
    ) ON DELETE RESTRICT,
    CONSTRAINT policy_decision_audit_novelty_fk FOREIGN KEY (
        novelty_evaluation_id, release_id, rule_set_id
    ) REFERENCES release_novelty_evaluations (
        id, release_id, policy_rule_set_id
    ) ON DELETE RESTRICT
);

CREATE INDEX policy_decision_audit_decision_idx
    ON policy_decision_audit (id, created_at DESC);
CREATE INDEX policy_decision_audit_release_idx
    ON policy_decision_audit (release_id, created_at DESC);

-- Append-only trigger: policy decisions are durable; the current pointer is
-- in policy_decisions, but history is never deleted.
CREATE FUNCTION reject_policy_decision_audit_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'policy decision audit rows are append-only';
END;
$$;

CREATE TRIGGER policy_decision_audit_append_only
BEFORE UPDATE OR DELETE ON policy_decision_audit
FOR EACH ROW EXECUTE FUNCTION reject_policy_decision_audit_rewrite();

-- Every authoritative current-row change writes its audit image inside the
-- same PostgreSQL statement. Direct SQL therefore cannot silently bypass
-- decision history.
CREATE FUNCTION audit_policy_decision_change() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    event_name text;
BEGIN
    IF TG_OP = 'INSERT' THEN
        event_name := 'created';
    ELSIF NEW.completion_state = 'completed' AND OLD.completion_state <> 'completed' THEN
        event_name := 'completed';
    ELSIF NEW.completion_state = 'failed' AND OLD.completion_state <> 'failed' THEN
        event_name := 'failed';
    ELSIF NEW.disposition IS DISTINCT FROM OLD.disposition
       OR NEW.actor IS DISTINCT FROM OLD.actor
       OR NEW.override_reason IS DISTINCT FROM OLD.override_reason THEN
        event_name := 'overridden';
    ELSE
        event_name := 'updated';
    END IF;

    INSERT INTO policy_decision_audit (
        id, classification_id, novelty_evaluation_id, release_id, work_item_id,
        rule_set_id, rule_set_version,
        matched_rule_ids, disposition, evidence_summary, source_fingerprint,
        actor, decided_at, completion_state, completed_at, override_reason,
        event, created_at
    ) VALUES (
        NEW.id, NEW.classification_id, NEW.novelty_evaluation_id,
        NEW.release_id, NEW.work_item_id, NEW.rule_set_id,
        NEW.rule_set_version, NEW.matched_rule_ids, NEW.disposition,
        NEW.evidence_summary, NEW.source_fingerprint, NEW.actor,
        NEW.decided_at, NEW.completion_state, NEW.completed_at,
        NEW.override_reason, event_name, clock_timestamp()
    );
    RETURN NEW;
END;
$$;

CREATE TRIGGER policy_decisions_audit
AFTER INSERT OR UPDATE ON policy_decisions
FOR EACH ROW EXECUTE FUNCTION audit_policy_decision_change();

CREATE FUNCTION reject_policy_decision_delete() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'policy decisions are durable and cannot be deleted';
END;
$$;

CREATE TRIGGER policy_decisions_no_delete
BEFORE DELETE ON policy_decisions
FOR EACH ROW EXECUTE FUNCTION reject_policy_decision_delete();

-- =========================================================================
-- Dry-run / test-against-catalog results (PRD-POLICY-008)
-- =========================================================================

CREATE TABLE policy_test_runs (
    id uuid PRIMARY KEY,
    rule_set_id uuid NOT NULL,
    rule_set_version integer NOT NULL,
    total_releases_evaluated integer NOT NULL CHECK (total_releases_evaluated >= 0),
    disposition_counts jsonb NOT NULL,
    matched_rule_counts jsonb NOT NULL,
    existing_disposition_counts jsonb NOT NULL,
    destructive_impact_count integer NOT NULL CHECK (destructive_impact_count >= 0),
    sample_release_ids uuid[] NOT NULL,
    ran_by jsonb NOT NULL,
    ran_at timestamptz NOT NULL,

    CONSTRAINT policy_test_runs_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT policy_test_runs_version_check CHECK (rule_set_version > 0),
    CONSTRAINT policy_test_runs_samples_check CHECK (cardinality(sample_release_ids) <= 20),
    CONSTRAINT policy_test_runs_counts_check CHECK (
        jsonb_typeof(disposition_counts) = 'array'
        AND jsonb_typeof(matched_rule_counts) = 'array'
        AND jsonb_typeof(existing_disposition_counts) = 'array'
        AND destructive_impact_count <= total_releases_evaluated
    ),
    CONSTRAINT policy_test_runs_actor_check CHECK (
        jsonb_typeof(ran_by) = 'object' AND (
            (ran_by->>'authority' = 'system'
             AND ran_by->>'policy_rule_set_version' = rule_set_version::text
             AND ran_by - ARRAY['authority', 'policy_rule_set_version'] = '{}'::jsonb)
            OR
            (ran_by->>'authority' = 'administrator'
             AND btrim(ran_by->>'actor') <> ''
             AND ran_by - ARRAY['authority', 'actor'] = '{}'::jsonb)
        )
    ),
    CONSTRAINT policy_test_runs_rule_fk FOREIGN KEY (rule_set_id, rule_set_version)
        REFERENCES policy_rule_sets (id, version) ON DELETE RESTRICT
);

CREATE FUNCTION validate_policy_test_run_summary() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    entry jsonb;
    bucket text;
    bucket_count bigint;
    proposed_total bigint := 0;
    existing_total bigint := 0;
    matched_total bigint := 0;
    delete_total bigint := 0;
    proposed_keys text[] := ARRAY[]::text[];
    existing_keys text[] := ARRAY[]::text[];
    matched_keys text[] := ARRAY[]::text[];
BEGIN
    IF NEW.total_releases_evaluated > 10000
       OR jsonb_array_length(NEW.disposition_counts) > 5
       OR jsonb_array_length(NEW.existing_disposition_counts) > 6
       OR jsonb_array_length(NEW.matched_rule_counts) > 1000
       OR cardinality(NEW.sample_release_ids) > LEAST(20, NEW.total_releases_evaluated)
       OR cardinality(NEW.sample_release_ids)
          <> (SELECT count(DISTINCT sample_id)
                FROM unnest(NEW.sample_release_ids) sample_id)
       OR EXISTS (
            SELECT 1 FROM unnest(NEW.sample_release_ids) sample_id
             LEFT JOIN releases release ON release.id = sample_id
            WHERE release.id IS NULL
               OR NOT COALESCE(uuid_extract_version(sample_id) = 7, false)
       ) THEN
        RAISE EXCEPTION 'invalid bounded policy test summary';
    END IF;

    FOR entry IN SELECT value FROM jsonb_array_elements(NEW.disposition_counts) LOOP
        IF jsonb_typeof(entry) <> 'array' OR jsonb_array_length(entry) <> 2
           OR jsonb_typeof(entry->0) <> 'string'
           OR jsonb_typeof(entry->1) <> 'number'
           OR entry->>1 !~ '^[1-9][0-9]*$' THEN
            RAISE EXCEPTION 'invalid proposed disposition bucket';
        END IF;
        bucket := entry->>0;
        bucket_count := (entry->>1)::bigint;
        IF bucket NOT IN ('accept', 'review', 'ignore', 'quarantine', 'delete')
           OR bucket = ANY(proposed_keys)
           OR bucket_count > NEW.total_releases_evaluated THEN
            RAISE EXCEPTION 'invalid proposed disposition bucket';
        END IF;
        proposed_keys := array_append(proposed_keys, bucket);
        proposed_total := proposed_total + bucket_count;
        IF bucket = 'delete' THEN
            delete_total := bucket_count;
        END IF;
    END LOOP;

    FOR entry IN SELECT value FROM jsonb_array_elements(NEW.existing_disposition_counts) LOOP
        IF jsonb_typeof(entry) <> 'array' OR jsonb_array_length(entry) <> 2
           OR jsonb_typeof(entry->0) NOT IN ('string', 'null')
           OR jsonb_typeof(entry->1) <> 'number'
           OR entry->>1 !~ '^[1-9][0-9]*$' THEN
            RAISE EXCEPTION 'invalid existing disposition bucket';
        END IF;
        bucket := CASE WHEN jsonb_typeof(entry->0) = 'null' THEN 'none' ELSE entry->>0 END;
        bucket_count := (entry->>1)::bigint;
        IF bucket NOT IN ('none', 'accept', 'review', 'ignore', 'quarantine', 'delete')
           OR bucket = ANY(existing_keys)
           OR bucket_count > NEW.total_releases_evaluated THEN
            RAISE EXCEPTION 'invalid existing disposition bucket';
        END IF;
        existing_keys := array_append(existing_keys, bucket);
        existing_total := existing_total + bucket_count;
    END LOOP;

    FOR entry IN SELECT value FROM jsonb_array_elements(NEW.matched_rule_counts) LOOP
        IF jsonb_typeof(entry) <> 'array' OR jsonb_array_length(entry) <> 2
           OR jsonb_typeof(entry->0) <> 'string' OR btrim(entry->>0) = ''
           OR jsonb_typeof(entry->1) <> 'number'
           OR entry->>1 !~ '^[1-9][0-9]*$' THEN
            RAISE EXCEPTION 'invalid matched policy rule bucket';
        END IF;
        bucket := entry->>0;
        bucket_count := (entry->>1)::bigint;
        IF bucket = ANY(matched_keys)
           OR bucket_count > NEW.total_releases_evaluated
           OR NOT EXISTS (
               SELECT 1 FROM policy_rule_sets rule_set,
                    jsonb_array_elements(rule_set.rules) rule
                WHERE rule_set.id = NEW.rule_set_id
                  AND rule_set.version = NEW.rule_set_version
                  AND rule->>'id' = bucket
           ) THEN
            RAISE EXCEPTION 'invalid matched policy rule bucket';
        END IF;
        matched_keys := array_append(matched_keys, bucket);
        matched_total := matched_total + bucket_count;
    END LOOP;

    IF proposed_total <> NEW.total_releases_evaluated
       OR existing_total <> NEW.total_releases_evaluated
       OR matched_total > NEW.total_releases_evaluated
       OR delete_total <> NEW.destructive_impact_count THEN
        RAISE EXCEPTION 'policy test summary counts are inconsistent';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER policy_test_runs_summary_integrity
BEFORE INSERT ON policy_test_runs
FOR EACH ROW EXECUTE FUNCTION validate_policy_test_run_summary();

CREATE TRIGGER policy_test_runs_immutable
BEFORE UPDATE OR DELETE ON policy_test_runs
FOR EACH ROW EXECUTE FUNCTION reject_policy_fact_rewrite();

CREATE INDEX policy_test_runs_rule_set_idx
    ON policy_test_runs (rule_set_id, ran_at DESC);
