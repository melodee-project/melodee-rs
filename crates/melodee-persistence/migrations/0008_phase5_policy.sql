-- Phase 5: Release classification, Track novelty, versioned policy rules,
-- durable decisions, and dry-run/test results.
--
-- This migration is additive. No statement reads, moves, rewrites, or deletes
-- media bytes or canonical storage paths. Classification and novelty are
-- pure observations stored as durable evidence. Policy decisions are recorded
-- before any destructive action; actual file deletion is a separate boundary.
-- Downgrade is safe: dropping the Phase 5 tables leaves catalog, identity, and
-- media facts intact.

-- =========================================================================
-- Release classification: multi-valued traits with explainable evidence
-- =========================================================================

CREATE TABLE release_classifications (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL UNIQUE REFERENCES releases (id) ON DELETE RESTRICT,
    confidence integer NOT NULL CHECK (confidence BETWEEN 0 AND 10000),
    classifier_version text NOT NULL,
    classified_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL
);

-- Multi-valued traits: a Release may have any combination of traits.
CREATE TABLE release_traits (
    classification_id uuid NOT NULL REFERENCES release_classifications (id) ON DELETE CASCADE,
    trait text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (classification_id, trait),
    CONSTRAINT release_traits_check CHECK (btrim(trait) <> '')
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
);

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
    UNIQUE (name, version)
);

CREATE INDEX policy_rule_sets_name_idx ON policy_rule_sets (name, version DESC);

-- Durable decision record that MUST exist before any destructive action
-- (PRD-POLICY-007, SPEC section 15).
CREATE TABLE policy_decisions (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL UNIQUE REFERENCES releases (id) ON DELETE RESTRICT,
    rule_set_id uuid NOT NULL REFERENCES policy_rule_sets (id) ON DELETE RESTRICT,
    rule_set_version integer NOT NULL,
    matched_rule_ids jsonb NOT NULL,
    disposition text NOT NULL,
    evidence_summary text NOT NULL,
    source_fingerprint text NULL,
    actor jsonb NOT NULL,
    decided_at timestamptz NOT NULL,
    completion_state text NOT NULL DEFAULT 'pending',
    completed_at timestamptz NULL,

    CONSTRAINT policy_decisions_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT policy_decisions_disposition_check CHECK (
        disposition IN ('accept', 'review', 'ignore', 'quarantine', 'delete')
    ),
    CONSTRAINT policy_decisions_completion_check CHECK (
        completion_state IN ('pending', 'completed', 'failed')
    ),
    CONSTRAINT policy_decisions_version_check CHECK (rule_set_version > 0),
    CONSTRAINT policy_decisions_summary_check CHECK (btrim(evidence_summary) <> '')
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
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    rule_set_id uuid NOT NULL REFERENCES policy_rule_sets (id) ON DELETE RESTRICT,
    rule_set_version integer NOT NULL,
    matched_rule_ids jsonb NOT NULL,
    disposition text NOT NULL,
    evidence_summary text NOT NULL,
    source_fingerprint text NULL,
    actor jsonb NOT NULL,
    decided_at timestamptz NOT NULL,
    completion_state text NOT NULL,
    completed_at timestamptz NULL,
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
    CONSTRAINT policy_decision_audit_event_check CHECK (
        event IN ('created', 'updated', 'completed', 'failed')
    )
);

CREATE INDEX policy_decision_audit_decision_idx
    ON policy_decision_audit (id, created_at DESC);
CREATE INDEX policy_decision_audit_release_idx
    ON policy_decision_audit (release_id, created_at DESC);

-- Append-only trigger: policy decisions are durable; the current pointer is
-- in policy_decisions, but history is never deleted.
CREATE FUNCTION reject_policy_decision_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'policy decision audit rows are append-only';
END;
$$;

CREATE TRIGGER policy_decision_audit_append_only
BEFORE UPDATE OR DELETE ON policy_decision_audit
FOR EACH ROW EXECUTE FUNCTION reject_policy_decision_update();

-- =========================================================================
-- Dry-run / test-against-catalog results (PRD-POLICY-008)
-- =========================================================================

CREATE TABLE policy_test_runs (
    id uuid PRIMARY KEY,
    rule_set_id uuid NOT NULL REFERENCES policy_rule_sets (id) ON DELETE RESTRICT,
    rule_set_version integer NOT NULL,
    total_releases_evaluated integer NOT NULL CHECK (total_releases_evaluated >= 0),
    disposition_counts jsonb NOT NULL,
    destructive_impact_count integer NOT NULL CHECK (destructive_impact_count >= 0),
    sample_release_ids uuid[] NOT NULL,
    ran_by jsonb NOT NULL,
    ran_at timestamptz NOT NULL,

    CONSTRAINT policy_test_runs_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT policy_test_runs_version_check CHECK (rule_set_version > 0)
);

CREATE INDEX policy_test_runs_rule_set_idx
    ON policy_test_runs (rule_set_id, ran_at DESC);
