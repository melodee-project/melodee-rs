-- Phase 4 authoritative catalog, Artist identity, AudioIdentity, semantic
-- overlays, participation projection, audit, and transactional outbox.
--
-- This migration is additive. Downgrade keeps all catalog and audit evidence;
-- a full rollback requires restoring a verified pre-migration database backup.
-- No statement in this migration reads, moves, rewrites, or deletes media.

CREATE TABLE artists (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    canonical_name text NOT NULL,
    normalized_name text NOT NULL,
    sort_name text NOT NULL,
    artist_type text NOT NULL,
    disambiguation text NULL,
    begin_date date NULL,
    end_date date NULL,
    status text NOT NULL,
    merged_into_artist_id uuid NULL REFERENCES artists (id) ON DELETE RESTRICT,
    revision bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT artists_name_check CHECK (
        btrim(canonical_name) <> '' AND btrim(normalized_name) <> '' AND btrim(sort_name) <> ''
    ),
    CONSTRAINT artists_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT artists_type_check CHECK (
        artist_type IN (
            'person', 'group', 'orchestra', 'ensemble', 'choir', 'character', 'other', 'unknown'
        )
    ),
    CONSTRAINT artists_status_check CHECK (
        status IN ('provisional', 'verified', 'merged', 'deprecated')
    ),
    CONSTRAINT artists_date_check CHECK (begin_date IS NULL OR end_date IS NULL OR begin_date <= end_date),
    CONSTRAINT artists_merge_check CHECK (
        (status = 'merged' AND merged_into_artist_id IS NOT NULL AND merged_into_artist_id <> id)
        OR (status <> 'merged' AND merged_into_artist_id IS NULL)
    ),
    CONSTRAINT artists_revision_check CHECK (revision > 0)
);

-- Deliberately non-unique: names are search attributes, never identity keys.
CREATE INDEX artists_normalized_name_idx ON artists (normalized_name, id);
CREATE INDEX artists_canonical_name_idx ON artists (canonical_name, id);
CREATE INDEX artists_merged_into_idx ON artists (merged_into_artist_id)
    WHERE merged_into_artist_id IS NOT NULL;

-- Resolve every redirect hop rather than assuming merges are one level deep.
-- A cycle is corruption and must fail closed instead of returning a plausible
-- but unstable identity.
CREATE FUNCTION resolve_artist_redirect(start_artist_id uuid) RETURNS uuid
LANGUAGE plpgsql STABLE STRICT AS $$
DECLARE
    current_artist_id uuid := start_artist_id;
    next_artist_id uuid;
    visited uuid[] := ARRAY[]::uuid[];
BEGIN
    LOOP
        IF current_artist_id = ANY(visited) THEN
            RAISE EXCEPTION 'Artist redirect cycle detected from %', start_artist_id
                USING ERRCODE = '23514';
        END IF;
        visited := array_append(visited, current_artist_id);
        SELECT merged_into_artist_id INTO next_artist_id
        FROM artists WHERE id = current_artist_id;
        IF NOT FOUND THEN
            RETURN NULL;
        END IF;
        IF next_artist_id IS NULL THEN
            RETURN current_artist_id;
        END IF;
        current_artist_id := next_artist_id;
    END LOOP;
END;
$$;

CREATE FUNCTION validate_artist_redirect_acyclic() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.merged_into_artist_id IS NOT NULL
       AND resolve_artist_redirect(NEW.merged_into_artist_id) = NEW.id
    THEN
        RAISE EXCEPTION 'Artist redirect cycle detected for %', NEW.id
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE FUNCTION lock_artist_redirect_graph() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(723491, 1);
    RETURN NEW;
END;
$$;

CREATE TRIGGER artists_redirect_graph_lock
BEFORE INSERT OR UPDATE OF merged_into_artist_id ON artists
FOR EACH ROW WHEN (NEW.merged_into_artist_id IS NOT NULL)
EXECUTE FUNCTION lock_artist_redirect_graph();

CREATE CONSTRAINT TRIGGER artists_redirect_acyclic_deferred
AFTER INSERT OR UPDATE ON artists
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW WHEN (NEW.merged_into_artist_id IS NOT NULL)
EXECUTE FUNCTION validate_artist_redirect_acyclic();

CREATE TABLE artist_aliases (
    id uuid PRIMARY KEY,
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    alias text NOT NULL,
    normalized_alias text NOT NULL,
    alias_type text NOT NULL,
    locale text NULL,
    evidence_source text NOT NULL,
    confidence integer NOT NULL,
    is_locked boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,

    CONSTRAINT artist_aliases_value_check CHECK (btrim(alias) <> '' AND btrim(normalized_alias) <> ''),
    CONSTRAINT artist_aliases_type_check CHECK (
        alias_type IN (
            'search', 'legal_name', 'former_name', 'translation', 'transliteration', 'other'
        )
    ),
    CONSTRAINT artist_aliases_confidence_check CHECK (confidence BETWEEN 0 AND 10000)
);

CREATE INDEX artist_aliases_lookup_idx ON artist_aliases (normalized_alias, artist_id);
CREATE INDEX artist_aliases_artist_idx ON artist_aliases (artist_id, id);

CREATE TABLE artist_external_identities (
    id uuid PRIMARY KEY,
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    entity_type text NOT NULL DEFAULT 'artist',
    provider text NOT NULL,
    external_id text NOT NULL,
    provider_unique_scope boolean NOT NULL DEFAULT false,
    confidence integer NOT NULL,
    evidence_source text NOT NULL,
    verified_at timestamptz NULL,
    is_locked boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,

    CONSTRAINT artist_external_identities_entity_check CHECK (entity_type = 'artist'),
    CONSTRAINT artist_external_identities_value_check CHECK (
        btrim(provider) <> '' AND btrim(external_id) <> ''
    ),
    CONSTRAINT artist_external_identities_confidence_check CHECK (confidence BETWEEN 0 AND 10000)
);

CREATE UNIQUE INDEX artist_external_identities_provider_unique_idx
    ON artist_external_identities (entity_type, provider, external_id)
    WHERE provider_unique_scope;
CREATE INDEX artist_external_identities_artist_idx
    ON artist_external_identities (artist_id, provider, id);
CREATE INDEX artist_external_identities_candidate_idx
    ON artist_external_identities (provider, external_id, artist_id);

CREATE TABLE artist_relationships (
    id uuid PRIMARY KEY,
    source_artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    target_artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    relationship_type text NOT NULL,
    role text NULL,
    start_date date NULL,
    end_date date NULL,
    confidence integer NOT NULL,
    evidence_source text NOT NULL,
    is_locked boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,

    CONSTRAINT artist_relationships_distinct_check CHECK (source_artist_id <> target_artist_id),
    CONSTRAINT artist_relationships_type_check CHECK (btrim(relationship_type) <> ''),
    CONSTRAINT artist_relationships_date_check CHECK (
        start_date IS NULL OR end_date IS NULL OR start_date <= end_date
    ),
    CONSTRAINT artist_relationships_confidence_check CHECK (confidence BETWEEN 0 AND 10000)
);

CREATE INDEX artist_relationships_source_idx
    ON artist_relationships (source_artist_id, relationship_type, target_artist_id);
CREATE INDEX artist_relationships_target_idx
    ON artist_relationships (target_artist_id, relationship_type, source_artist_id);

CREATE TABLE group_memberships (
    id uuid PRIMARY KEY,
    member_artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    group_artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    role text NOT NULL,
    start_date date NULL,
    end_date date NULL,
    confidence integer NOT NULL,
    evidence_source text NOT NULL,
    is_locked boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,

    CONSTRAINT group_memberships_distinct_check CHECK (member_artist_id <> group_artist_id),
    CONSTRAINT group_memberships_role_check CHECK (btrim(role) <> ''),
    CONSTRAINT group_memberships_date_check CHECK (
        start_date IS NULL OR end_date IS NULL OR start_date <= end_date
    ),
    CONSTRAINT group_memberships_confidence_check CHECK (confidence BETWEEN 0 AND 10000)
);

CREATE INDEX group_memberships_group_dates_idx
    ON group_memberships (group_artist_id, start_date, end_date, member_artist_id);
CREATE INDEX group_memberships_member_idx
    ON group_memberships (member_artist_id, group_artist_id, id);

CREATE TABLE audio_identities (
    id uuid PRIMARY KEY,
    status text NOT NULL,
    merged_into_audio_identity_id uuid NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    revision bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT audio_identities_status_check CHECK (
        status IN ('provisional', 'verified', 'merged', 'deprecated')
    ),
    CONSTRAINT audio_identities_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT audio_identities_merge_check CHECK (
        (status = 'merged' AND merged_into_audio_identity_id IS NOT NULL
            AND merged_into_audio_identity_id <> id)
        OR (status <> 'merged' AND merged_into_audio_identity_id IS NULL)
    ),
    CONSTRAINT audio_identities_revision_check CHECK (revision > 0)
);

CREATE FUNCTION resolve_audio_identity_redirect(start_audio_identity_id uuid) RETURNS uuid
LANGUAGE plpgsql STABLE STRICT AS $$
DECLARE
    current_audio_identity_id uuid := start_audio_identity_id;
    next_audio_identity_id uuid;
    visited uuid[] := ARRAY[]::uuid[];
BEGIN
    LOOP
        IF current_audio_identity_id = ANY(visited) THEN
            RAISE EXCEPTION 'AudioIdentity redirect cycle detected from %', start_audio_identity_id
                USING ERRCODE = '23514';
        END IF;
        visited := array_append(visited, current_audio_identity_id);
        SELECT merged_into_audio_identity_id INTO next_audio_identity_id
        FROM audio_identities WHERE id = current_audio_identity_id;
        IF NOT FOUND THEN
            RETURN NULL;
        END IF;
        IF next_audio_identity_id IS NULL THEN
            RETURN current_audio_identity_id;
        END IF;
        current_audio_identity_id := next_audio_identity_id;
    END LOOP;
END;
$$;

CREATE FUNCTION validate_audio_identity_redirect_acyclic() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.merged_into_audio_identity_id IS NOT NULL
       AND resolve_audio_identity_redirect(NEW.merged_into_audio_identity_id) = NEW.id
    THEN
        RAISE EXCEPTION 'AudioIdentity redirect cycle detected for %', NEW.id
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$;

CREATE FUNCTION lock_audio_identity_redirect_graph() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(723491, 2);
    RETURN NEW;
END;
$$;

CREATE TRIGGER audio_identities_redirect_graph_lock
BEFORE INSERT OR UPDATE OF merged_into_audio_identity_id ON audio_identities
FOR EACH ROW WHEN (NEW.merged_into_audio_identity_id IS NOT NULL)
EXECUTE FUNCTION lock_audio_identity_redirect_graph();

CREATE CONSTRAINT TRIGGER audio_identities_redirect_acyclic_deferred
AFTER INSERT OR UPDATE ON audio_identities
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW WHEN (NEW.merged_into_audio_identity_id IS NOT NULL)
EXECUTE FUNCTION validate_audio_identity_redirect_acyclic();

CREATE TABLE audio_identity_evidence (
    id uuid PRIMARY KEY,
    source_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    candidate_audio_identity_id uuid NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    track_id uuid NULL,
    evidence_kind text NOT NULL,
    polarity text NOT NULL,
    confidence integer NOT NULL,
    source text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT audio_identity_evidence_distinct_check CHECK (
        candidate_audio_identity_id IS NULL
        OR source_audio_identity_id <> candidate_audio_identity_id
    ),
    CONSTRAINT audio_identity_evidence_kind_check CHECK (btrim(evidence_kind) <> ''),
    CONSTRAINT audio_identity_evidence_source_check CHECK (btrim(source) <> ''),
    CONSTRAINT audio_identity_evidence_polarity_check CHECK (
        polarity IN ('supports', 'conflicts')
    ),
    CONSTRAINT audio_identity_evidence_confidence_check CHECK (confidence BETWEEN 0 AND 10000)
);

CREATE INDEX audio_identity_evidence_source_idx
    ON audio_identity_evidence (source_audio_identity_id, candidate_audio_identity_id, id);

CREATE TABLE audio_identity_match_decisions (
    id uuid PRIMARY KEY,
    source_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    candidate_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    outcome text NOT NULL,
    authority text NOT NULL,
    is_locked boolean NOT NULL DEFAULT false,
    reason text NOT NULL,
    predecessor_decision_id uuid NULL REFERENCES audio_identity_match_decisions (id) ON DELETE RESTRICT,
    evidence_ids jsonb NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT audio_identity_match_decisions_distinct_check CHECK (
        source_audio_identity_id <> candidate_audio_identity_id
    ),
    CONSTRAINT audio_identity_match_decisions_outcome_check CHECK (
        outcome IN ('proposed', 'confirmed')
    ),
    CONSTRAINT audio_identity_match_decisions_authority_check CHECK (
        authority IN ('automated', 'admin')
    ),
    CONSTRAINT audio_identity_match_decisions_lock_check CHECK (
        NOT is_locked OR authority = 'admin'
    ),
    CONSTRAINT audio_identity_match_decisions_reason_check CHECK (btrim(reason) <> ''),
    CONSTRAINT audio_identity_match_decisions_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    UNIQUE (id, source_audio_identity_id, candidate_audio_identity_id)
);

CREATE TABLE audio_identity_match_current (
    source_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    candidate_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    decision_id uuid NOT NULL UNIQUE,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (source_audio_identity_id, candidate_audio_identity_id),
    CONSTRAINT audio_identity_match_current_decision_fk FOREIGN KEY (
        decision_id, source_audio_identity_id, candidate_audio_identity_id
    ) REFERENCES audio_identity_match_decisions (
        id, source_audio_identity_id, candidate_audio_identity_id
    ) ON DELETE RESTRICT
);

-- One root and one successor per predecessor make the append-only decision
-- history a chain. Repository operations additionally serialize on stable
-- AudioIdentity rows so concurrent first decisions cannot race the NULL root.
CREATE UNIQUE INDEX audio_identity_match_decisions_one_root_idx
    ON audio_identity_match_decisions (
        source_audio_identity_id, candidate_audio_identity_id
    ) WHERE predecessor_decision_id IS NULL;
CREATE UNIQUE INDEX audio_identity_match_decisions_one_successor_idx
    ON audio_identity_match_decisions (predecessor_decision_id)
    WHERE predecessor_decision_id IS NOT NULL;

CREATE TABLE releases (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    title text NOT NULL,
    normalized_title text NOT NULL,
    sort_title text NOT NULL,
    display_artist text NULL,
    release_date date NULL,
    release_date_text text NULL,
    status text NOT NULL DEFAULT 'active',
    revision bigint NOT NULL DEFAULT 1,
    current_overlay_id uuid NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT releases_title_check CHECK (
        btrim(title) <> '' AND btrim(normalized_title) <> '' AND btrim(sort_title) <> ''
    ),
    CONSTRAINT releases_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT releases_status_check CHECK (status IN ('active', 'deprecated')),
    CONSTRAINT releases_revision_check CHECK (revision > 0)
);

CREATE INDEX releases_normalized_title_idx ON releases (normalized_title, id);

CREATE TABLE tracks (
    id uuid PRIMARY KEY,
    public_api_id uuid NOT NULL UNIQUE,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    playable_media_asset_id uuid NOT NULL,
    playable_purpose text GENERATED ALWAYS AS ('playable'::text) STORED,
    ordinal integer NOT NULL,
    title text NOT NULL,
    normalized_title text NOT NULL,
    sort_title text NOT NULL,
    display_artist text NULL,
    disc_number integer NULL,
    track_number integer NULL,
    duration_ms bigint NULL,
    source_path text NULL,
    revision bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT tracks_id_v7_check CHECK (COALESCE(uuid_extract_version(id) = 7, false)),
    CONSTRAINT tracks_ordinal_check CHECK (ordinal > 0),
    CONSTRAINT tracks_title_check CHECK (
        btrim(title) <> '' AND btrim(normalized_title) <> '' AND btrim(sort_title) <> ''
    ),
    CONSTRAINT tracks_disc_check CHECK (disc_number IS NULL OR disc_number > 0),
    CONSTRAINT tracks_number_check CHECK (track_number IS NULL OR track_number > 0),
    CONSTRAINT tracks_duration_check CHECK (duration_ms IS NULL OR duration_ms > 0),
    CONSTRAINT tracks_revision_check CHECK (revision > 0),
    UNIQUE (release_id, ordinal),
    UNIQUE (id, release_id)
);

ALTER TABLE audio_identity_evidence
    ADD CONSTRAINT audio_identity_evidence_track_fk FOREIGN KEY (track_id)
        REFERENCES tracks (id) ON DELETE RESTRICT;

CREATE TABLE audio_identity_merge_events (
    id uuid PRIMARY KEY,
    source_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    target_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    actor text NOT NULL,
    reason text NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT audio_identity_merge_events_distinct_check CHECK (
        source_audio_identity_id <> target_audio_identity_id
    ),
    CONSTRAINT audio_identity_merge_events_value_check CHECK (
        btrim(actor) <> '' AND btrim(reason) <> ''
    )
);

CREATE TABLE audio_identity_split_events (
    id uuid PRIMARY KEY,
    source_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    new_audio_identity_id uuid NOT NULL REFERENCES audio_identities (id) ON DELETE RESTRICT,
    actor text NOT NULL,
    reason text NOT NULL,
    selected_track_ids jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT audio_identity_split_events_distinct_check CHECK (
        source_audio_identity_id <> new_audio_identity_id
    ),
    CONSTRAINT audio_identity_split_events_value_check CHECK (
        btrim(actor) <> '' AND btrim(reason) <> ''
    )
);

CREATE INDEX tracks_release_order_idx ON tracks (release_id, ordinal, id);
CREATE INDEX tracks_audio_identity_idx ON tracks (audio_identity_id, id);
CREATE INDEX tracks_normalized_title_idx ON tracks (normalized_title, id);

CREATE TABLE media_assets (
    id uuid PRIMARY KEY,
    canonical_relative_path text NOT NULL UNIQUE,
    byte_hash text NOT NULL,
    byte_size bigint NOT NULL,
    mime_type text NOT NULL,
    container text NULL,
    codec text NULL,
    sample_rate_hz integer NULL,
    channels integer NULL,
    bitrate_bps bigint NULL,
    duration_ms bigint NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT media_assets_path_check CHECK (
        canonical_relative_path !~ '(^/|(^|/)\.\.(/|$)|\\)' AND btrim(canonical_relative_path) <> ''
    ),
    CONSTRAINT media_assets_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT media_assets_hash_check CHECK (byte_hash ~ '^[0-9a-f]{64}$'),
    CONSTRAINT media_assets_size_check CHECK (byte_size >= 0),
    CONSTRAINT media_assets_technical_check CHECK (
        (sample_rate_hz IS NULL OR sample_rate_hz > 0)
        AND (channels IS NULL OR channels > 0)
        AND (bitrate_bps IS NULL OR bitrate_bps > 0)
        AND (duration_ms IS NULL OR duration_ms > 0)
    )
);

-- Byte hashes are evidence and intentionally non-unique.
CREATE INDEX media_assets_byte_hash_idx ON media_assets (byte_hash, byte_size, id);

CREATE TABLE track_media_assets (
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    media_asset_id uuid NOT NULL REFERENCES media_assets (id) ON DELETE RESTRICT,
    purpose text NOT NULL,
    ordinal integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL,

    PRIMARY KEY (track_id, media_asset_id, purpose),
    CONSTRAINT track_media_assets_purpose_check CHECK (purpose IN ('playable', 'alternate')),
    CONSTRAINT track_media_assets_ordinal_check CHECK (ordinal > 0)
);

CREATE UNIQUE INDEX track_media_assets_one_primary_idx
    ON track_media_assets (track_id) WHERE purpose = 'playable';
CREATE INDEX track_media_assets_asset_idx ON track_media_assets (media_asset_id, track_id);

-- The Track names its mandatory playable asset, while this deferred circular
-- FK permits set-based Track-before-link insertion inside one transaction.
-- Together with the partial unique index it proves exactly one playable link
-- declaratively, without row-level validation queries.
ALTER TABLE tracks
    ADD CONSTRAINT tracks_exactly_one_playable_asset_fk FOREIGN KEY (
        id, playable_media_asset_id, playable_purpose
    ) REFERENCES track_media_assets (track_id, media_asset_id, purpose)
      ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE observed_artists (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    track_id uuid NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    scope text NOT NULL,
    source_ordinal integer NOT NULL,
    raw_value text NOT NULL,
    normalized_value text NOT NULL,
    display_value text NOT NULL,
    source_pointer text NOT NULL,
    source_payload jsonb NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT observed_artists_scope_check CHECK (
        (scope = 'release' AND track_id IS NULL) OR (scope = 'track' AND track_id IS NOT NULL)
    ),
    CONSTRAINT observed_artists_value_check CHECK (
        btrim(raw_value) <> '' AND btrim(normalized_value) <> ''
            AND btrim(display_value) <> '' AND btrim(source_pointer) <> ''
    ),
    CONSTRAINT observed_artists_ordinal_check CHECK (source_ordinal >= 0),
    UNIQUE (release_id, scope, source_pointer, source_ordinal),
    UNIQUE (id, release_id),
    UNIQUE (id, track_id),
    UNIQUE (id, release_id, track_id),
    CONSTRAINT observed_artists_track_release_fk FOREIGN KEY (track_id, release_id)
        REFERENCES tracks (id, release_id) ON DELETE RESTRICT
);

CREATE INDEX observed_artists_lookup_idx ON observed_artists (normalized_value, id);
CREATE INDEX observed_artists_release_idx ON observed_artists (release_id, track_id, source_ordinal);

CREATE TABLE release_artists (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    observed_artist_id uuid NULL REFERENCES observed_artists (id) ON DELETE RESTRICT,
    role text NOT NULL,
    presentation_order integer NOT NULL,
    display_value text NOT NULL,
    join_phrase text NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT release_artists_observation_fk FOREIGN KEY (observed_artist_id, release_id)
        REFERENCES observed_artists (id, release_id) ON DELETE RESTRICT,
    CONSTRAINT release_artists_role_check CHECK (
        role IN ('primary', 'featured', 'guest', 'other')
    ),
    CONSTRAINT release_artists_order_check CHECK (presentation_order > 0),
    CONSTRAINT release_artists_display_check CHECK (btrim(display_value) <> ''),
    UNIQUE (release_id, presentation_order)
);

CREATE INDEX release_artists_artist_idx ON release_artists (artist_id, release_id);

CREATE TABLE track_artists (
    id uuid PRIMARY KEY,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    observed_artist_id uuid NULL REFERENCES observed_artists (id) ON DELETE RESTRICT,
    role text NOT NULL,
    presentation_order integer NOT NULL,
    display_value text NOT NULL,
    join_phrase text NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT track_artists_observation_fk FOREIGN KEY (observed_artist_id, track_id)
        REFERENCES observed_artists (id, track_id) ON DELETE RESTRICT,
    CONSTRAINT track_artists_role_check CHECK (
        role IN ('primary', 'featured', 'guest', 'other')
    ),
    CONSTRAINT track_artists_order_check CHECK (presentation_order > 0),
    CONSTRAINT track_artists_display_check CHECK (btrim(display_value) <> ''),
    UNIQUE (track_id, presentation_order)
);

CREATE INDEX track_artists_artist_idx ON track_artists (artist_id, track_id);

CREATE TABLE contributions (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    track_id uuid NULL,
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    observed_artist_id uuid NULL REFERENCES observed_artists (id) ON DELETE RESTRICT,
    role text NOT NULL,
    role_detail text NULL,
    evidence_source text NOT NULL,
    display_value text NULL,
    presentation_order integer NOT NULL DEFAULT 0,
    is_performance boolean NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT contributions_track_release_fk FOREIGN KEY (track_id, release_id)
        REFERENCES tracks (id, release_id) ON DELETE RESTRICT,
    CONSTRAINT contributions_observation_fk FOREIGN KEY (observed_artist_id, release_id, track_id)
        REFERENCES observed_artists (id, release_id, track_id) ON DELETE RESTRICT,
    CONSTRAINT contributions_role_check CHECK (
        btrim(role) <> '' AND btrim(evidence_source) <> ''
    ),
    CONSTRAINT contributions_order_check CHECK (presentation_order >= 0)
);

CREATE INDEX contributions_artist_track_idx
    ON contributions (artist_id, is_performance, track_id, release_id);
CREATE INDEX contributions_release_idx ON contributions (release_id, track_id, id);

-- Nullable Track ownership needs `IS NOT DISTINCT FROM`; an ordinary
-- composite FK uses MATCH SIMPLE and would not validate Release-level rows.
CREATE FUNCTION validate_contribution_observation_scope() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.observed_artist_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM observed_artists observed
        WHERE observed.id = NEW.observed_artist_id
          AND observed.release_id = NEW.release_id
          AND observed.track_id IS NOT DISTINCT FROM NEW.track_id
    ) THEN
        RAISE EXCEPTION 'contribution observation is outside its Release/Track scope';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER contributions_observation_scope
BEFORE INSERT OR UPDATE ON contributions
FOR EACH ROW EXECUTE FUNCTION validate_contribution_observation_scope();

CREATE TABLE artist_match_evidence (
    id uuid PRIMARY KEY,
    observed_artist_id uuid NOT NULL REFERENCES observed_artists (id) ON DELETE RESTRICT,
    candidate_artist_id uuid NULL REFERENCES artists (id) ON DELETE RESTRICT,
    evidence_kind text NOT NULL,
    polarity text NOT NULL,
    confidence integer NOT NULL,
    source text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT artist_match_evidence_kind_check CHECK (btrim(evidence_kind) <> ''),
    CONSTRAINT artist_match_evidence_polarity_check CHECK (
        polarity IN ('supports', 'conflicts')
    ),
    CONSTRAINT artist_match_evidence_confidence_check CHECK (confidence BETWEEN 0 AND 10000)
);

CREATE INDEX artist_match_evidence_observation_idx
    ON artist_match_evidence (observed_artist_id, candidate_artist_id, created_at, id);

CREATE TABLE artist_match_decisions (
    id uuid PRIMARY KEY,
    observed_artist_id uuid NOT NULL REFERENCES observed_artists (id) ON DELETE RESTRICT,
    artist_id uuid NULL REFERENCES artists (id) ON DELETE RESTRICT,
    outcome text NOT NULL,
    authority text NOT NULL,
    confidence integer NOT NULL,
    reason text NOT NULL,
    is_locked boolean NOT NULL DEFAULT false,
    predecessor_decision_id uuid NULL REFERENCES artist_match_decisions (id) ON DELETE RESTRICT,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT artist_match_decisions_outcome_check CHECK (
        outcome IN ('proposed', 'confirmed')
    ),
    CONSTRAINT artist_match_decisions_authority_check CHECK (
        authority IN ('automated', 'admin')
    ),
    CONSTRAINT artist_match_decisions_artist_check CHECK (
        artist_id IS NOT NULL
    ),
    CONSTRAINT artist_match_decisions_lock_check CHECK (NOT is_locked OR authority = 'admin'),
    CONSTRAINT artist_match_decisions_confidence_check CHECK (confidence BETWEEN 0 AND 10000),
    CONSTRAINT artist_match_decisions_reason_check CHECK (btrim(reason) <> ''),
    UNIQUE (id, observed_artist_id)
);

CREATE TABLE artist_match_current (
    observed_artist_id uuid PRIMARY KEY REFERENCES observed_artists (id) ON DELETE RESTRICT,
    decision_id uuid NOT NULL UNIQUE,
    updated_at timestamptz NOT NULL,
    CONSTRAINT artist_match_current_decision_fk FOREIGN KEY (decision_id, observed_artist_id)
        REFERENCES artist_match_decisions (id, observed_artist_id) ON DELETE RESTRICT
);

-- The current pointer intentionally tracks only authoritative decisions, so
-- chain integrity is enforced independently for proposal history.
CREATE UNIQUE INDEX artist_match_decisions_one_root_idx
    ON artist_match_decisions (observed_artist_id)
    WHERE predecessor_decision_id IS NULL;
CREATE UNIQUE INDEX artist_match_decisions_one_successor_idx
    ON artist_match_decisions (predecessor_decision_id)
    WHERE predecessor_decision_id IS NOT NULL;

CREATE TABLE artist_negative_assertions (
    id uuid PRIMARY KEY,
    observed_artist_id uuid NOT NULL REFERENCES observed_artists (id) ON DELETE RESTRICT,
    candidate_artist_id uuid NULL REFERENCES artists (id) ON DELETE RESTRICT,
    candidate_provider text NULL,
    candidate_external_id text NULL,
    assertion_kind text NOT NULL DEFAULT 'do_not_match',
    predecessor_assertion_id uuid NULL REFERENCES artist_negative_assertions (id) ON DELETE RESTRICT,
    authority text NOT NULL,
    reason text NOT NULL,
    is_locked boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,

    CONSTRAINT artist_negative_assertions_kind_check CHECK (
        assertion_kind IN ('do_not_match', 'revoke')
    ),
    CONSTRAINT artist_negative_assertions_candidate_check CHECK (
        (candidate_artist_id IS NOT NULL
            AND candidate_provider IS NULL AND candidate_external_id IS NULL)
        OR (candidate_artist_id IS NULL
            AND btrim(candidate_provider) <> '' AND btrim(candidate_external_id) <> '')
    ),
    CONSTRAINT artist_negative_assertions_authority_check CHECK (
        authority IN ('automated', 'admin')
    ),
    CONSTRAINT artist_negative_assertions_reason_check CHECK (btrim(reason) <> ''),
    CONSTRAINT artist_negative_assertions_lock_check CHECK (NOT is_locked OR authority = 'admin'),
    CONSTRAINT artist_negative_assertions_revoke_check CHECK (
        (assertion_kind = 'do_not_match' AND predecessor_assertion_id IS NULL)
        OR (assertion_kind = 'revoke' AND predecessor_assertion_id IS NOT NULL)
    )
);

CREATE INDEX artist_negative_assertions_lookup_idx
    ON artist_negative_assertions (observed_artist_id, candidate_artist_id, created_at DESC, id DESC);
CREATE INDEX artist_negative_assertions_provider_idx
    ON artist_negative_assertions (
        observed_artist_id, candidate_provider, candidate_external_id, created_at DESC, id DESC
    );
CREATE UNIQUE INDEX artist_negative_assertions_one_local_root_idx
    ON artist_negative_assertions (observed_artist_id, candidate_artist_id)
    WHERE predecessor_assertion_id IS NULL AND candidate_artist_id IS NOT NULL;
CREATE UNIQUE INDEX artist_negative_assertions_one_provider_root_idx
    ON artist_negative_assertions (
        observed_artist_id, candidate_provider, candidate_external_id
    ) WHERE predecessor_assertion_id IS NULL AND candidate_provider IS NOT NULL;
CREATE UNIQUE INDEX artist_negative_assertions_one_successor_idx
    ON artist_negative_assertions (predecessor_assertion_id)
    WHERE predecessor_assertion_id IS NOT NULL;

CREATE TABLE group_membership_track_exclusions (
    membership_id uuid NOT NULL REFERENCES group_memberships (id) ON DELETE RESTRICT,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    reason text NOT NULL,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (membership_id, track_id),
    CONSTRAINT group_membership_track_exclusions_reason_check CHECK (btrim(reason) <> '')
);

CREATE TABLE artist_track_associations (
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE RESTRICT,
    association_type text NOT NULL,
    participation_kind text NOT NULL,
    source_key uuid NOT NULL,
    source_artist_id uuid NULL REFERENCES artists (id) ON DELETE RESTRICT,
    confidence integer NOT NULL,
    is_performance boolean NOT NULL,
    created_at timestamptz NOT NULL,

    PRIMARY KEY (artist_id, track_id, association_type, participation_kind, source_key),
    CONSTRAINT artist_track_associations_kind_check CHECK (
        participation_kind IN ('direct', 'inferred_group')
    ),
    CONSTRAINT artist_track_associations_type_check CHECK (btrim(association_type) <> ''),
    CONSTRAINT artist_track_associations_confidence_check CHECK (confidence BETWEEN 0 AND 10000)
);

CREATE INDEX artist_track_associations_default_idx
    ON artist_track_associations (artist_id, track_id)
    WHERE is_performance;
CREATE INDEX artist_track_associations_all_idx
    ON artist_track_associations (artist_id, is_performance, track_id);
CREATE INDEX artist_track_associations_track_idx
    ON artist_track_associations (track_id, artist_id);

ALTER TABLE release_publication_receipts
    ADD CONSTRAINT release_publication_receipts_item_release_uniq UNIQUE (work_item_id, release_id);
ALTER TABLE release_work_item_manifests
    ADD CONSTRAINT release_work_item_manifests_item_id_uniq UNIQUE (work_item_id, id);

CREATE TABLE catalog_imports (
    id uuid PRIMARY KEY,
    work_item_id uuid NOT NULL UNIQUE,
    release_id uuid NOT NULL UNIQUE REFERENCES releases (id) ON DELETE RESTRICT,
    manifest_row_id bigint NOT NULL UNIQUE,
    manifest_id uuid NOT NULL,
    manifest_schema_version integer NOT NULL,
    semantic_manifest_hash text NOT NULL,
    publication_plan_hash text NOT NULL,
    sealed_manifest_hash text NOT NULL,
    resolver_version text NOT NULL,
    imported_at timestamptz NOT NULL,

    CONSTRAINT catalog_imports_receipt_fk FOREIGN KEY (work_item_id, release_id)
        REFERENCES release_publication_receipts (work_item_id, release_id) ON DELETE RESTRICT,
    CONSTRAINT catalog_imports_manifest_fk FOREIGN KEY (work_item_id, manifest_row_id)
        REFERENCES release_work_item_manifests (work_item_id, id) ON DELETE RESTRICT,
    CONSTRAINT catalog_imports_version_check CHECK (manifest_schema_version = 3),
    CONSTRAINT catalog_imports_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT catalog_imports_hash_check CHECK (
        semantic_manifest_hash ~ '^[0-9a-f]{64}$'
        AND publication_plan_hash ~ '^[0-9a-f]{64}$'
        AND sealed_manifest_hash ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT catalog_imports_resolver_check CHECK (btrim(resolver_version) <> '')
);

CREATE TABLE catalog_semantic_overlays (
    id uuid PRIMARY KEY,
    release_id uuid NOT NULL REFERENCES releases (id) ON DELETE RESTRICT,
    catalog_import_id uuid NOT NULL REFERENCES catalog_imports (id) ON DELETE RESTRICT,
    predecessor_overlay_id uuid NULL REFERENCES catalog_semantic_overlays (id) ON DELETE RESTRICT,
    schema_name text NOT NULL,
    schema_version integer NOT NULL,
    source_manifest_id uuid NOT NULL,
    source_semantic_manifest_hash text NOT NULL,
    publication_plan_hash text NOT NULL,
    resolver_version text NOT NULL,
    payload jsonb NOT NULL,
    overlay_hash text NOT NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT catalog_semantic_overlays_schema_check CHECK (
        schema_name = 'catalog.semantic' AND schema_version = 1
    ),
    CONSTRAINT catalog_semantic_overlays_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT catalog_semantic_overlays_hash_check CHECK (
        source_semantic_manifest_hash ~ '^[0-9a-f]{64}$'
        AND publication_plan_hash ~ '^[0-9a-f]{64}$'
        AND overlay_hash ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT catalog_semantic_overlays_resolver_check CHECK (btrim(resolver_version) <> ''),
    UNIQUE (release_id, overlay_hash),
    UNIQUE (id, release_id)
);

ALTER TABLE releases
    ADD CONSTRAINT releases_current_overlay_fk FOREIGN KEY (current_overlay_id, id)
        REFERENCES catalog_semantic_overlays (id, release_id) ON DELETE RESTRICT;

CREATE INDEX catalog_semantic_overlays_release_idx
    ON catalog_semantic_overlays (release_id, created_at DESC, id DESC);

CREATE TABLE artist_merge_events (
    id uuid PRIMARY KEY,
    source_artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    target_artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    actor text NOT NULL,
    reason text NOT NULL,
    moved_relationships jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT artist_merge_events_distinct_check CHECK (source_artist_id <> target_artist_id),
    CONSTRAINT artist_merge_events_reason_check CHECK (btrim(actor) <> '' AND btrim(reason) <> '')
);

CREATE TABLE artist_split_events (
    id uuid PRIMARY KEY,
    source_artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    new_artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE RESTRICT,
    actor text NOT NULL,
    reason text NOT NULL,
    selected_assignments jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT artist_split_events_distinct_check CHECK (source_artist_id <> new_artist_id),
    CONSTRAINT artist_split_events_reason_check CHECK (btrim(actor) <> '' AND btrim(reason) <> '')
);

CREATE TABLE catalog_audit_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_id uuid NOT NULL UNIQUE,
    entity_type text NOT NULL,
    entity_id uuid NOT NULL,
    event_type text NOT NULL,
    actor text NOT NULL,
    reason text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT catalog_audit_events_value_check CHECK (
        btrim(entity_type) <> '' AND btrim(event_type) <> ''
            AND btrim(actor) <> '' AND btrim(reason) <> ''
    )
);

CREATE INDEX catalog_audit_events_entity_idx
    ON catalog_audit_events (entity_type, entity_id, created_at, id);

CREATE TABLE artist_catalog_aggregates (
    artist_id uuid PRIMARY KEY REFERENCES artists (id) ON DELETE RESTRICT,
    release_count bigint NOT NULL,
    billed_track_count bigint NOT NULL,
    performance_track_count bigint NOT NULL,
    all_credit_track_count bigint NOT NULL,
    revision bigint NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT artist_catalog_aggregates_counts_check CHECK (
        release_count >= 0 AND billed_track_count >= 0
            AND performance_track_count >= 0 AND all_credit_track_count >= 0
            AND revision > 0
    )
);

CREATE TABLE release_catalog_aggregates (
    release_id uuid PRIMARY KEY REFERENCES releases (id) ON DELETE RESTRICT,
    track_count bigint NOT NULL,
    duration_ms bigint NOT NULL,
    media_bytes bigint NOT NULL,
    revision bigint NOT NULL,
    updated_at timestamptz NOT NULL,
    CONSTRAINT release_catalog_aggregates_counts_check CHECK (
        track_count >= 0 AND duration_ms >= 0 AND media_bytes >= 0 AND revision > 0
    )
);

CREATE TABLE catalog_projection_outbox (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    projection_name text NOT NULL,
    entity_type text NOT NULL,
    entity_id uuid NOT NULL,
    operation text NOT NULL,
    entity_revision bigint NOT NULL,
    available_at timestamptz NOT NULL,
    lease_owner text NULL,
    lease_expires_at timestamptz NULL,
    attempt_count integer NOT NULL DEFAULT 0,
    processed_at timestamptz NULL,
    last_error text NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT catalog_projection_outbox_projection_check CHECK (btrim(projection_name) <> ''),
    CONSTRAINT catalog_projection_outbox_entity_check CHECK (btrim(entity_type) <> ''),
    CONSTRAINT catalog_projection_outbox_operation_check CHECK (operation IN ('upsert', 'delete')),
    CONSTRAINT catalog_projection_outbox_revision_check CHECK (entity_revision > 0),
    CONSTRAINT catalog_projection_outbox_attempt_check CHECK (attempt_count >= 0),
    CONSTRAINT catalog_projection_outbox_lease_check CHECK (
        (lease_owner IS NULL AND lease_expires_at IS NULL)
        OR (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
    ),
    UNIQUE (projection_name, entity_type, entity_id, entity_revision)
);

CREATE INDEX catalog_projection_outbox_pending_idx
    ON catalog_projection_outbox (projection_name, available_at, id)
    WHERE processed_at IS NULL;

CREATE TABLE legacy_catalog_id_mappings (
    id uuid PRIMARY KEY,
    migration_run_id uuid NOT NULL,
    entity_type text NOT NULL,
    legacy_numeric_id bigint NOT NULL,
    internal_id uuid NOT NULL,
    public_api_id uuid NOT NULL,
    source_snapshot text NOT NULL,
    imported_at timestamptz NOT NULL,

    CONSTRAINT legacy_catalog_id_mappings_id_v7_check CHECK (
        COALESCE(uuid_extract_version(id) = 7, false)
    ),
    CONSTRAINT legacy_catalog_id_mappings_entity_check CHECK (
        entity_type IN ('artist', 'release', 'track')
    ),
    CONSTRAINT legacy_catalog_id_mappings_snapshot_check CHECK (btrim(source_snapshot) <> ''),
    UNIQUE (migration_run_id, entity_type, legacy_numeric_id),
    UNIQUE (migration_run_id, entity_type, public_api_id)
);

CREATE INDEX legacy_catalog_id_mappings_internal_idx
    ON legacy_catalog_id_mappings (entity_type, internal_id);

CREATE INDEX release_work_items_catalog_claim_idx
    ON release_work_items (updated_at, id)
    WHERE state IN ('published', 'cataloging');

-- Catalog retry remains in Cataloging so it converges from PostgreSQL v3 and
-- never returns to inspection. Cataloged is an unleased completion boundary;
-- a delayed Cataloging retry is the only legal unleased Cataloging shape.
ALTER TABLE release_work_items
    DROP CONSTRAINT release_work_items_retry_consistency_check;
ALTER TABLE release_work_items
    ADD CONSTRAINT release_work_items_retry_consistency_check CHECK (
        next_attempt_at IS NULL OR state IN ('failed', 'cataloging')
    );
ALTER TABLE release_work_items
    ADD CONSTRAINT release_work_items_catalog_lease_check CHECK (
        (state <> 'cataloged'
            OR (lease_owner IS NULL AND lease_acquired_at IS NULL AND lease_expires_at IS NULL))
        AND (state <> 'cataloging'
            OR (lease_owner IS NOT NULL
                AND lease_acquired_at IS NOT NULL AND lease_expires_at IS NOT NULL)
            OR (lease_owner IS NULL
                AND lease_acquired_at IS NULL AND lease_expires_at IS NULL))
    );

-- Once publication has upgraded the authoritative manifest to v3, Phase 4
-- consumes it as immutable input. Rewriting any sealed field fails closed.
CREATE FUNCTION reject_published_manifest_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.schema_version = 3 AND (
        NEW.schema_version IS DISTINCT FROM OLD.schema_version
        OR NEW.manifest_hash IS DISTINCT FROM OLD.manifest_hash
        OR NEW.manifest_payload IS DISTINCT FROM OLD.manifest_payload
    ) THEN
        RAISE EXCEPTION 'published manifest v3 is immutable for work item %', OLD.work_item_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER release_work_item_manifests_v3_immutable
BEFORE UPDATE ON release_work_item_manifests
FOR EACH ROW EXECUTE FUNCTION reject_published_manifest_rewrite();

-- Public API identifiers are compatibility data. They may differ from the
-- internal UUIDv7 primary key, but cannot be regenerated after publication or
-- legacy import (ADR-0117).
CREATE FUNCTION reject_public_api_id_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.public_api_id IS DISTINCT FROM OLD.public_api_id THEN
        RAISE EXCEPTION 'public API id is immutable for % row %', TG_TABLE_NAME, OLD.id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER artists_public_api_id_immutable
BEFORE UPDATE ON artists
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();
CREATE TRIGGER releases_public_api_id_immutable
BEFORE UPDATE ON releases
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();
CREATE TRIGGER tracks_public_api_id_immutable
BEFORE UPDATE ON tracks
FOR EACH ROW EXECUTE FUNCTION reject_public_api_id_rewrite();

-- Immutable evidence tables accept inserts only. Current pointers and
-- normalized catalog rows remain mutable through fenced repository operations.
CREATE FUNCTION reject_catalog_evidence_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'catalog evidence table % is append-only', TG_TABLE_NAME;
END;
$$;

CREATE TRIGGER catalog_imports_append_only
BEFORE UPDATE OR DELETE ON catalog_imports
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER catalog_semantic_overlays_append_only
BEFORE UPDATE OR DELETE ON catalog_semantic_overlays
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER artist_match_evidence_append_only
BEFORE UPDATE OR DELETE ON artist_match_evidence
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER artist_match_decisions_append_only
BEFORE UPDATE OR DELETE ON artist_match_decisions
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER artist_negative_assertions_append_only
BEFORE UPDATE OR DELETE ON artist_negative_assertions
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER artist_merge_events_append_only
BEFORE UPDATE OR DELETE ON artist_merge_events
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER artist_split_events_append_only
BEFORE UPDATE OR DELETE ON artist_split_events
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER audio_identity_evidence_append_only
BEFORE UPDATE OR DELETE ON audio_identity_evidence
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER audio_identity_match_decisions_append_only
BEFORE UPDATE OR DELETE ON audio_identity_match_decisions
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER audio_identity_merge_events_append_only
BEFORE UPDATE OR DELETE ON audio_identity_merge_events
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER audio_identity_split_events_append_only
BEFORE UPDATE OR DELETE ON audio_identity_split_events
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER catalog_audit_events_append_only
BEFORE UPDATE OR DELETE ON catalog_audit_events
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
CREATE TRIGGER legacy_catalog_id_mappings_append_only
BEFORE UPDATE OR DELETE ON legacy_catalog_id_mappings
FOR EACH ROW EXECUTE FUNCTION reject_catalog_evidence_mutation();
