-- ADR-0213: full manifest evidence lives on persistent file storage.
--
-- PostgreSQL keeps the logical manifest row, its version, semantic and sealed
-- hashes, one verified content-addressed reference, and compact query facts.
-- The payload column becomes nullable so a verified reference can replace the
-- inline JSONB; it is never dropped here, so a partially upgraded installation
-- keeps the recoverable source until an independent verification finalizes it.

CREATE TABLE release_work_item_manifest_evidence (
    manifest_row_id bigint PRIMARY KEY
        REFERENCES release_work_item_manifests (id) ON DELETE CASCADE,
    asset_key text NOT NULL REFERENCES asset_files (asset_key),
    storage_kind text NOT NULL,
    content_sha256 text NOT NULL,
    content_length bigint NOT NULL,
    serialization_version integer NOT NULL,
    revision integer NOT NULL,
    source text NOT NULL,
    created_at timestamptz NOT NULL,
    verified_at timestamptz NOT NULL,

    CONSTRAINT release_work_item_manifest_evidence_kind_check CHECK (
        storage_kind IN ('object', 'chunked')
    ),
    CONSTRAINT release_work_item_manifest_evidence_digest_check CHECK (
        content_sha256 ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT release_work_item_manifest_evidence_length_check CHECK (content_length > 0),
    CONSTRAINT release_work_item_manifest_evidence_version_check CHECK (
        serialization_version > 0 AND revision > 0
    ),
    CONSTRAINT release_work_item_manifest_evidence_source_check CHECK (
        source IN ('written', 'backfilled')
    )
);

CREATE INDEX release_work_item_manifest_evidence_key_idx
    ON release_work_item_manifest_evidence (asset_key);

-- Append-only ownership history. A superseded reference keeps its parts (and
-- shared parts) alive under the store's documented retention policy, and its
-- filesystem cost is reported rather than silently reclaimed.
CREATE TABLE release_work_item_manifest_evidence_revision (
    manifest_row_id bigint NOT NULL
        REFERENCES release_work_item_manifests (id) ON DELETE CASCADE,
    revision integer NOT NULL,
    asset_key text NOT NULL REFERENCES asset_files (asset_key),
    storage_kind text NOT NULL,
    content_sha256 text NOT NULL,
    content_length bigint NOT NULL,
    source text NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT release_work_item_manifest_evidence_revision_kind_check CHECK (
        storage_kind IN ('object', 'chunked')
    ),
    CONSTRAINT release_work_item_manifest_evidence_revision_digest_check CHECK (
        content_sha256 ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT release_work_item_manifest_evidence_revision_length_check CHECK (
        content_length > 0 AND revision > 0
    ),
    CONSTRAINT release_work_item_manifest_evidence_revision_source_check CHECK (
        source IN ('written', 'backfilled')
    ),
    PRIMARY KEY (manifest_row_id, revision)
);

CREATE INDEX release_work_item_manifest_evidence_revision_key_idx
    ON release_work_item_manifest_evidence_revision (asset_key);

-- Compact relational query facts. Browse, search, policy batches and Admin
-- listings use these columns and never read a full manifest file.
CREATE TABLE release_work_item_manifest_facts (
    manifest_row_id bigint PRIMARY KEY
        REFERENCES release_work_item_manifests (id) ON DELETE CASCADE,
    release_title text NULL,
    source_path text NULL,
    has_corrupt_media boolean NOT NULL DEFAULT false,
    structured_traits text[] NOT NULL DEFAULT '{}',
    embedded_traits text[] NOT NULL DEFAULT '{}',
    lyrics_present boolean NOT NULL DEFAULT false,
    serde_default_applied boolean NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL,

    CONSTRAINT release_work_item_manifest_facts_traits_check CHECK (
        array_position(structured_traits, NULL) IS NULL
        AND array_position(embedded_traits, NULL) IS NULL
    )
);

CREATE TABLE release_work_item_manifest_track_facts (
    manifest_row_id bigint NOT NULL
        REFERENCES release_work_item_manifests (id) ON DELETE CASCADE,
    relative_path text NOT NULL,
    bit_depth integer NULL,
    sample_rate_hz integer NULL,
    channels integer NULL,
    fingerprint_digest text NULL,
    fingerprint_length integer NULL,
    corrupt boolean NOT NULL DEFAULT false,
    genre text NULL,
    source_ordinal integer NOT NULL DEFAULT 0,
    observed_at timestamptz NOT NULL,

    CONSTRAINT release_work_item_manifest_track_facts_path_check CHECK (
        btrim(relative_path) <> ''
    ),
    CONSTRAINT release_work_item_manifest_track_facts_depth_check CHECK (
        bit_depth IS NULL OR bit_depth > 0
    ),
    CONSTRAINT release_work_item_manifest_track_facts_digest_check CHECK (
        fingerprint_digest IS NULL OR fingerprint_digest ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT release_work_item_manifest_track_facts_ordinal_check CHECK (
        source_ordinal >= 0
    ),
    PRIMARY KEY (manifest_row_id, relative_path)
);

CREATE INDEX release_work_item_manifest_track_facts_path_idx
    ON release_work_item_manifest_track_facts (relative_path);

-- Resumable, auditable upgrade progress. One row per installation, and the
-- cumulative per-row receipts are the evidence the verify command compares
-- against.
CREATE TABLE manifest_evidence_migration (
    singleton boolean PRIMARY KEY DEFAULT true,
    store_id uuid NOT NULL,
    started_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    completed_at timestamptz NULL,
    manifests_total bigint NOT NULL DEFAULT 0,
    manifests_migrated bigint NOT NULL DEFAULT 0,
    bytes_migrated bigint NOT NULL DEFAULT 0,
    verification_errors bigint NOT NULL DEFAULT 0,
    last_manifest_row_id bigint NOT NULL DEFAULT 0,

    CONSTRAINT manifest_evidence_migration_singleton_check CHECK (singleton),
    CONSTRAINT manifest_evidence_migration_counts_check CHECK (
        manifests_total >= 0 AND manifests_migrated >= 0 AND bytes_migrated >= 0
        AND verification_errors >= 0 AND last_manifest_row_id >= 0
    ),
    CONSTRAINT manifest_evidence_migration_completion_check CHECK (
        (completed_at IS NULL AND verification_errors = 0)
        OR completed_at IS NOT NULL
    )
);

CREATE TABLE manifest_evidence_migration_receipts (
    manifest_row_id bigint PRIMARY KEY,
    content_sha256 text NOT NULL,
    content_length bigint NOT NULL,
    derived_facts_hash text NOT NULL,
    migrated_at timestamptz NOT NULL,

    CONSTRAINT manifest_evidence_migration_receipts_digest_check CHECK (
        content_sha256 ~ '^[0-9a-f]{64}$' AND derived_facts_hash ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT manifest_evidence_migration_receipts_length_check CHECK (content_length > 0)
);

-- The payload is nullable from here on: a verified reference replaces it.
ALTER TABLE release_work_item_manifests ALTER COLUMN manifest_payload DROP NOT NULL;

-- Sealed v3 manifests were immutable, including their payload. Clearing the
-- payload is now allowed only when the row already carries a verified evidence
-- reference for exactly that manifest hash; every other rewrite still fails.
CREATE OR REPLACE FUNCTION reject_published_manifest_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.schema_version = 3 AND (
        NEW.schema_version IS DISTINCT FROM OLD.schema_version
        OR NEW.manifest_hash IS DISTINCT FROM OLD.manifest_hash
        OR (
            NEW.manifest_payload IS DISTINCT FROM OLD.manifest_payload
            AND NOT (
                NEW.manifest_payload IS NULL
                AND OLD.manifest_payload IS NOT NULL
                AND EXISTS (
                    SELECT 1 FROM release_work_item_manifest_evidence evidence
                     WHERE evidence.manifest_row_id = OLD.id
                )
            )
        )
    ) THEN
        RAISE EXCEPTION 'published manifest v3 is immutable for work item %', OLD.work_item_id;
    END IF;
    RETURN NEW;
END;
$$;

-- ADR-0213/ADR-0165: the genre serving projection is a rebuildable projection
-- over immutable evidence. It now reads the compact track facts instead of
-- traversing the manifest payload, so import no longer touches inline JSON.
CREATE OR REPLACE FUNCTION project_catalog_import_genres()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO catalog_genre_terms (
        track_id, release_id, surface, display_value, source_order
    )
    SELECT DISTINCT ON (track.id, btrim(track_fact.genre))
           track.id, track.release_id, 'native',
           btrim(track_fact.genre), track_fact.source_ordinal
    FROM tracks track
    JOIN release_work_item_manifest_track_facts track_fact
      ON track_fact.manifest_row_id = NEW.manifest_row_id
     AND track_fact.relative_path = track.source_path
    WHERE track.release_id = NEW.release_id
      AND btrim(COALESCE(track_fact.genre, '')) <> ''
    ORDER BY track.id, btrim(track_fact.genre), track_fact.source_ordinal
    ON CONFLICT DO NOTHING;

    INSERT INTO catalog_genre_terms (
        track_id, release_id, surface, display_value, source_order
    )
    SELECT DISTINCT ON (track.id, genre_part.value)
           track.id, track.release_id, 'opensubsonic', genre_part.value,
           track_fact.source_ordinal * 100000 + genre_part.ordinality
    FROM tracks track
    JOIN release_work_item_manifest_track_facts track_fact
      ON track_fact.manifest_row_id = NEW.manifest_row_id
     AND track_fact.relative_path = track.source_path
    CROSS JOIN LATERAL unnest(string_to_array(track_fact.genre, ';'))
        WITH ORDINALITY genre_part(value, ordinality)
    WHERE track.release_id = NEW.release_id
      AND btrim(genre_part.value) <> ''
    ORDER BY track.id, genre_part.value,
             track_fact.source_ordinal * 100000 + genre_part.ordinality
    ON CONFLICT DO NOTHING;
    RETURN NEW;
END;
$$;
