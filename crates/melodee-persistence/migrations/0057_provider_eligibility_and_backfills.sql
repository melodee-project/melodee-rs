-- ADR-0212: revisioned provider eligibility, bounded scheduling and durable
-- backfills.
--
-- The installation records only effective booleans per external provider. It
-- never records credentials: those stay in settings (environment, config file
-- or admin overrides) and are resolved by the capability resolver. An empty
-- `provider_eligibility` table means "not yet resolved"; scheduling then keeps
-- the pre-ADR behavior (every known provider) so a fresh installation reaches
-- playable media and catches up through the first reconciliation.

CREATE TABLE provider_eligibility (
    provider text PRIMARY KEY,
    revision bigint NOT NULL,
    metadata_eligible boolean NOT NULL,
    artwork_eligible boolean NOT NULL,
    observed_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT provider_eligibility_provider_check CHECK (
        provider IN ('brave', 'deezer', 'discogs', 'itunes', 'lastfm',
                     'metalapi', 'musicbrainz', 'spotify', 'wikidata')
    ),
    CONSTRAINT provider_eligibility_revision_check CHECK (revision > 0)
);

-- One installation-wide revision counter. Every effective change increments
-- it, and a provider revision is the value observed at its last change. A
-- backfill is keyed by that revision so repeated refreshes and concurrent
-- reconcilers cannot duplicate a scan.
CREATE TABLE provider_eligibility_state (
    singleton boolean PRIMARY KEY DEFAULT true,
    revision bigint NOT NULL DEFAULT 0,
    refreshed_at timestamptz NULL,
    last_error text NULL,
    last_error_at timestamptz NULL,

    CONSTRAINT provider_eligibility_state_singleton_check CHECK (singleton),
    CONSTRAINT provider_eligibility_state_revision_check CHECK (revision >= 0)
);

INSERT INTO provider_eligibility_state (singleton, revision) VALUES (true, 0);

-- Dormant work is historically attempted work suspended by disablement. It is
-- excluded from runnable counts and claims, keeps its task identity, attempt
-- history and retry budget, and becomes runnable exactly once when the provider
-- turns eligible again.
ALTER TABLE provider_enrichment_tasks DROP CONSTRAINT provider_enrichment_status_check;
ALTER TABLE provider_enrichment_tasks ADD CONSTRAINT provider_enrichment_status_check CHECK (
    status IN ('queued', 'running', 'succeeded', 'retry', 'failed', 'dormant')
);
ALTER TABLE provider_enrichment_tasks
    ADD COLUMN dormant_at timestamptz NULL,
    ADD COLUMN provider_revision bigint NULL,
    ADD CONSTRAINT provider_enrichment_dormant_check CHECK (
        (status = 'dormant' AND dormant_at IS NOT NULL AND lease_owner IS NULL)
        OR (status <> 'dormant' AND dormant_at IS NULL)
    );

-- The claim index keeps only runnable and recoverable shapes: queued/retry
-- waiting for their next attempt, and running rows whose lease expired. So the
-- long-term scan does not grow with completed or dormant history.
DROP INDEX provider_enrichment_claim_idx;
CREATE INDEX provider_enrichment_claim_idx
    ON provider_enrichment_tasks (next_attempt_at, updated_at, id)
    WHERE status IN ('queued', 'retry')
       OR (status = 'running' AND lease_expires_at IS NOT NULL);
CREATE INDEX provider_enrichment_provider_state_idx
    ON provider_enrichment_tasks (provider, status, entity_type);

-- One durable backfill per provider revision and entity type. Artists and
-- Releases advance independently by stable keyset so a page is a bounded,
-- restartable unit that never scans the whole catalog.
CREATE TABLE provider_enrichment_backfills (
    id uuid PRIMARY KEY,
    provider text NOT NULL,
    provider_revision bigint NOT NULL,
    entity_type text NOT NULL,
    status text NOT NULL DEFAULT 'running',
    last_entity_id uuid NULL,
    entities_scanned bigint NOT NULL DEFAULT 0,
    tasks_enqueued bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    completed_at timestamptz NULL,

    CONSTRAINT provider_enrichment_backfills_entity_check CHECK (
        entity_type IN ('artist', 'release')
    ),
    CONSTRAINT provider_enrichment_backfills_status_check CHECK (
        status IN ('running', 'completed', 'fenced')
    ),
    CONSTRAINT provider_enrichment_backfills_counts_check CHECK (
        entities_scanned >= 0 AND tasks_enqueued >= 0
    ),
    CONSTRAINT provider_enrichment_backfills_completion_check CHECK (
        (status = 'completed' AND completed_at IS NOT NULL)
        OR (status <> 'completed' AND completed_at IS NULL)
    ),
    UNIQUE (provider, provider_revision, entity_type)
);

CREATE INDEX provider_enrichment_backfills_active_idx
    ON provider_enrichment_backfills (status, provider, entity_type);

-- A merged Artist must not be re-enriched. The backfill keyset walks live
-- identity only; this partial index keeps that page cheap.
CREATE INDEX artists_live_backfill_idx
    ON artists (id)
    WHERE status IN ('active', 'verified');
CREATE INDEX releases_live_backfill_idx
    ON releases (id)
    WHERE status = 'active';
