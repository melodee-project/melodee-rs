-- ADR-0148: legacy migration contract — durable run model and the
-- original-release-date preservation column.
--
-- Everything here is additive and backward compatible: a new schema with four
-- tables plus one nullable column on `releases`. No existing row is altered.
-- Rollback plan: this migration is forward-only per repository rule; reverting
-- it (drop schema + drop the nullable column) touches no production data
-- because only the migration tooling writes these records.

CREATE SCHEMA legacy_migration;

-- Durable migration runs. State transitions and fencing follow ADR-0148 §D.6:
-- `owner` + `lease_expires_at` fence active advancement exactly like the
-- work-item engine; a stale owner cannot transition or mutate a run.
CREATE TABLE legacy_migration.runs (
    id uuid PRIMARY KEY,
    state text NOT NULL,
    source_identity text NOT NULL,
    source_snapshot_marker text NOT NULL,
    owner text NULL,
    lease_expires_at timestamptz NULL,
    notes jsonb NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    completed_at timestamptz NULL,

    CONSTRAINT runs_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT runs_state_check CHECK (
        state IN (
            'planned', 'importing', 'verifying', 'delta-ready',
            'cutover-ready', 'complete', 'failed', 'cancelled'
        )
    ),
    CONSTRAINT runs_source_identity_check CHECK (btrim(source_identity) <> ''),
    CONSTRAINT runs_snapshot_marker_check CHECK (btrim(source_snapshot_marker) <> '')
);

-- Per-domain progress: bounded keyset checkpoint, counters, and elapsed time.
-- Retries re-apply batches keyed by source identity + source row evidence, so
-- process death between target insert and checkpoint update is idempotent.
CREATE TABLE legacy_migration.checkpoints (
    run_id uuid NOT NULL REFERENCES legacy_migration.runs (id) ON DELETE RESTRICT,
    domain text NOT NULL,
    last_source_pk bigint NOT NULL,
    processed bigint NOT NULL,
    inserted bigint NOT NULL,
    updated bigint NOT NULL,
    issues bigint NOT NULL,
    elapsed_ms bigint NOT NULL,
    updated_at timestamptz NOT NULL,

    PRIMARY KEY (run_id, domain),
    CONSTRAINT checkpoints_domain_check CHECK (btrim(domain) <> ''),
    CONSTRAINT checkpoints_counts_check CHECK (
        last_source_pk >= 0 AND processed >= 0 AND inserted >= 0
        AND updated >= 0 AND issues >= 0 AND elapsed_ms >= 0
    )
);

-- Public-ID and source-row mappings for non-catalog domains. Catalog
-- artist/release/track mappings live in `legacy_catalog_id_mappings` with
-- the same per-run uniqueness guarantees; this table keeps the remaining
-- domains (users, playlists, shares, podcasts, radio, …) out of semantic
-- catalog identity exactly as ADR-0148 §D.2 requires.
CREATE TABLE legacy_migration.entity_map (
    run_id uuid NOT NULL REFERENCES legacy_migration.runs (id) ON DELETE RESTRICT,
    domain text NOT NULL,
    source_pk bigint NOT NULL,
    source_public_id uuid NULL,
    target_id uuid NOT NULL,
    source_updated_at timestamptz NULL,
    row_hash text NULL,
    created_at timestamptz NOT NULL,

    PRIMARY KEY (run_id, domain, source_pk),
    CONSTRAINT entity_map_domain_check CHECK (btrim(domain) <> ''),
    CONSTRAINT entity_map_row_hash_check CHECK (
        row_hash IS NULL OR row_hash ~ '^[0-9a-f]{64}$'
    )
);

CREATE UNIQUE INDEX entity_map_target_idx
    ON legacy_migration.entity_map (domain, target_id);

-- Audit trail for every unrepresentable or anomalous source row. Nothing is
-- silently dropped: an unimportable row is an issue, not an omission.
CREATE TABLE legacy_migration.issues (
    id uuid PRIMARY KEY,
    run_id uuid NOT NULL REFERENCES legacy_migration.runs (id) ON DELETE RESTRICT,
    domain text NOT NULL,
    source_pk bigint NULL,
    severity text NOT NULL,
    code text NOT NULL,
    message text NOT NULL,
    detail jsonb NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT issues_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT issues_severity_check CHECK (severity IN ('info', 'warning', 'error')),
    CONSTRAINT issues_code_check CHECK (btrim(code) <> ''),
    CONSTRAINT issues_message_check CHECK (btrim(message) <> '')
);

CREATE INDEX issues_run_domain_idx
    ON legacy_migration.issues (run_id, domain, severity);

-- ADR-0148 §D.8: legacy `Albums.OriginalReleaseDate` is preserved separately
-- from the edition/release date. Nullable; existing rows are untouched.
ALTER TABLE releases
    ADD COLUMN original_release_date date NULL;
