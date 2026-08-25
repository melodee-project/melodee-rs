-- ADR-0162: durable fail-degraded provider enrichment and Release external evidence.
-- Additive only. Rollback is a verified database restore; source media is never touched.

CREATE TABLE provider_enrichment_tasks (
    id uuid PRIMARY KEY,
    entity_type text NOT NULL,
    entity_id uuid NOT NULL,
    provider text NOT NULL,
    purpose text NOT NULL,
    request_key text NOT NULL,
    status text NOT NULL DEFAULT 'queued',
    attempt_count integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL,
    lease_owner text NULL,
    lease_acquired_at timestamptz NULL,
    lease_expires_at timestamptz NULL,
    last_error_category text NULL,
    last_error_message text NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    completed_at timestamptz NULL,
    CONSTRAINT provider_enrichment_entity_check CHECK (entity_type IN ('artist', 'release')),
    CONSTRAINT provider_enrichment_provider_check CHECK (
        provider IN ('brave', 'deezer', 'discogs', 'itunes', 'lastfm',
                     'metalapi', 'musicbrainz', 'spotify', 'wikidata', 'melodee')
    ),
    CONSTRAINT provider_enrichment_purpose_check CHECK (
        purpose IN ('artist_metadata', 'release_metadata', 'artist_artwork', 'release_artwork')
    ),
    CONSTRAINT provider_enrichment_request_key_check CHECK (
        btrim(request_key) <> '' AND char_length(request_key) <= 128
    ),
    CONSTRAINT provider_enrichment_status_check CHECK (
        status IN ('queued', 'running', 'succeeded', 'retry', 'failed')
    ),
    CONSTRAINT provider_enrichment_attempt_check CHECK (attempt_count >= 0 AND attempt_count <= 20),
    CONSTRAINT provider_enrichment_lease_check CHECK (
        (status = 'running' AND lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR status <> 'running'
    ),
    UNIQUE (entity_type, entity_id, provider, purpose, request_key)
);

CREATE INDEX provider_enrichment_claim_idx
    ON provider_enrichment_tasks (status, next_attempt_at, updated_at, id);
CREATE INDEX provider_enrichment_entity_idx
    ON provider_enrichment_tasks (entity_type, entity_id, updated_at DESC, id DESC);

CREATE TABLE provider_enrichment_attempts (
    id uuid PRIMARY KEY,
    task_id uuid NOT NULL REFERENCES provider_enrichment_tasks (id) ON DELETE RESTRICT,
    attempt_number integer NOT NULL,
    provider text NOT NULL,
    purpose text NOT NULL,
    outcome text NOT NULL,
    error_category text NULL,
    error_message text NULL,
    cache_key text NULL,
    started_at timestamptz NOT NULL,
    completed_at timestamptz NOT NULL,
    CONSTRAINT provider_enrichment_attempt_number_check CHECK (attempt_number > 0),
    CONSTRAINT provider_enrichment_attempt_outcome_check CHECK (
        outcome IN ('cache_hit', 'success', 'retry', 'failed', 'disabled')
    ),
    CONSTRAINT provider_enrichment_attempt_error_check CHECK (
        (outcome IN ('success', 'cache_hit', 'disabled') AND error_category IS NULL)
        OR outcome IN ('retry', 'failed')
    ),
    UNIQUE (task_id, attempt_number)
);

CREATE INDEX provider_enrichment_attempts_task_idx
    ON provider_enrichment_attempts (task_id, started_at DESC, id DESC);

CREATE TABLE release_external_identities (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    provider text NOT NULL,
    external_id text NOT NULL,
    trusted boolean NOT NULL DEFAULT false,
    confidence integer NOT NULL,
    evidence_source text NOT NULL,
    is_locked boolean NOT NULL DEFAULT false,
    observed_at timestamptz NOT NULL,
    CONSTRAINT release_external_identity_provider_check CHECK (
        provider IN ('brave', 'deezer', 'discogs', 'itunes', 'lastfm',
                     'metalapi', 'musicbrainz', 'spotify', 'wikidata', 'melodee')
    ),
    CONSTRAINT release_external_identity_value_check CHECK (btrim(external_id) <> ''),
    CONSTRAINT release_external_identity_confidence_check CHECK (confidence BETWEEN 0 AND 10000)
);

CREATE INDEX release_external_identity_lookup_idx
    ON release_external_identities (provider, external_id, trusted, release_id);
CREATE INDEX release_external_identity_release_idx
    ON release_external_identities (release_id, provider, observed_at DESC, id DESC);

CREATE UNIQUE INDEX release_external_identity_exact_idx
    ON release_external_identities (release_id, provider, external_id);

-- Provider artwork is evidence, never an administrator override. Serving
-- code checks catalog_entity_images first, so refreshing this table cannot
-- overwrite or outrank a locked/operator-owned image.
CREATE TABLE catalog_provider_images (
    id uuid PRIMARY KEY,
    entity_type text NOT NULL,
    entity_id uuid NOT NULL,
    provider text NOT NULL,
    source_url text NOT NULL,
    image_mime_type text NOT NULL,
    image_bytes bytea NOT NULL,
    confidence integer NOT NULL,
    observed_at timestamptz NOT NULL,
    CONSTRAINT catalog_provider_image_entity_check CHECK (entity_type IN ('artist','release')),
    CONSTRAINT catalog_provider_image_provider_check CHECK (
        provider IN ('brave', 'deezer', 'discogs', 'itunes', 'lastfm',
                     'metalapi', 'musicbrainz', 'spotify', 'wikidata', 'melodee')
    ),
    CONSTRAINT catalog_provider_image_url_check CHECK (btrim(source_url) <> ''),
    CONSTRAINT catalog_provider_image_mime_check CHECK (image_mime_type IN ('image/jpeg','image/png')),
    CONSTRAINT catalog_provider_image_size_check CHECK (octet_length(image_bytes) BETWEEN 1 AND 10485760),
    CONSTRAINT catalog_provider_image_confidence_check CHECK (confidence BETWEEN 0 AND 10000),
    UNIQUE (entity_type, entity_id, provider)
);

CREATE INDEX catalog_provider_images_lookup_idx
    ON catalog_provider_images (entity_type, entity_id, confidence DESC, observed_at DESC, id DESC);

-- Search results are candidate evidence, not trusted external identities. A
-- query may return several provider candidates for one local Artist, so this
-- table retains rank/query/payload instead of attaching every result to the
-- Artist's canonical external-identity rows.
CREATE TABLE provider_identity_candidates (
    id uuid PRIMARY KEY,
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    provider text NOT NULL,
    external_id text NOT NULL,
    query text NOT NULL,
    candidate_rank integer NOT NULL,
    confidence integer NOT NULL,
    payload jsonb NOT NULL,
    observed_at timestamptz NOT NULL,
    CONSTRAINT provider_identity_candidate_provider_check CHECK (
        provider IN ('brave', 'deezer', 'discogs', 'itunes', 'lastfm',
                     'metalapi', 'musicbrainz', 'spotify', 'wikidata')
    ),
    CONSTRAINT provider_identity_candidate_value_check CHECK (
        btrim(external_id) <> '' AND btrim(query) <> ''
    ),
    CONSTRAINT provider_identity_candidate_rank_check CHECK (candidate_rank > 0),
    CONSTRAINT provider_identity_candidate_confidence_check CHECK (confidence BETWEEN 0 AND 10000),
    CONSTRAINT provider_identity_candidate_payload_check CHECK (octet_length(payload::text) <= 65536),
    UNIQUE (artist_id, provider, external_id, query)
);

CREATE INDEX provider_identity_candidates_lookup_idx
    ON provider_identity_candidates (artist_id, provider, observed_at DESC, candidate_rank);

-- ADR-0160/0162: Release convergence retains the source Release and the
-- exact Track ordinal map so an operator can reverse one audited merge.
ALTER TABLE releases
    ADD COLUMN merged_into_release_id uuid NULL REFERENCES releases (id) ON DELETE RESTRICT;
ALTER TABLE releases DROP CONSTRAINT releases_status_check;
ALTER TABLE releases ADD CONSTRAINT releases_status_check CHECK (
    status IN ('active', 'deprecated', 'deleted', 'merged')
);
ALTER TABLE releases ADD CONSTRAINT releases_merge_check CHECK (
    (status = 'merged' AND merged_into_release_id IS NOT NULL AND merged_into_release_id <> id)
    OR (status <> 'merged' AND merged_into_release_id IS NULL)
);
CREATE INDEX releases_merged_into_idx ON releases (merged_into_release_id)
    WHERE merged_into_release_id IS NOT NULL;

CREATE TABLE release_merge_events (
    id uuid PRIMARY KEY,
    source_release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    target_release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    actor text NOT NULL,
    reason text NOT NULL,
    moved_tracks jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT release_merge_events_distinct_check CHECK (source_release_id <> target_release_id),
    CONSTRAINT release_merge_events_values_check CHECK (btrim(actor) <> '' AND btrim(reason) <> '')
);

CREATE TABLE release_split_events (
    id uuid PRIMARY KEY,
    merge_event_id uuid NOT NULL REFERENCES release_merge_events (id) ON DELETE RESTRICT,
    source_release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    target_release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    actor text NOT NULL,
    reason text NOT NULL,
    restored_tracks jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT release_split_events_distinct_check CHECK (source_release_id <> target_release_id),
    CONSTRAINT release_split_events_values_check CHECK (btrim(actor) <> '' AND btrim(reason) <> '')
);

-- Track ownership is part of the reversible Release merge correction. These
-- composite references must be deferred so Track, observation, and
-- contribution Release coordinates can move as one transaction.
ALTER TABLE observed_artists
    ALTER CONSTRAINT observed_artists_track_release_fk DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE contributions
    ALTER CONSTRAINT contributions_track_release_fk DEFERRABLE INITIALLY DEFERRED,
    ALTER CONSTRAINT contributions_observation_fk DEFERRABLE INITIALLY DEFERRED;
