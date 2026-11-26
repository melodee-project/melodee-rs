-- PERF-004 / RM-09: durable affected-identity reconciliation seeds.
--
-- Ordinary imports previously ran five catalog-wide identity queries after
-- every successful album. That work is not album-proportional and, at scale,
-- turns a catalog build into a quadratic scan. This table makes the *affected*
-- identity facts durable at the authoritative mutation, so reconciliation can
-- coalesce many imports into one bounded closure pass and still converge after
-- a crash or a worker restart.
--
-- The closure itself still runs the complete candidate algorithms (so it is
-- equivalent to the global oracle); the seeds are the durable delivery
-- obligation and the coalescing boundary. A low-frequency global backstop
-- remains for installations that predate the seeds or restore a backup.
--
-- Additive only. A binary downgrade ignores the table; an empty queue simply
-- means the periodic backstop is the only convergence path.

CREATE TABLE catalog_identity_reconciliation_seed (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    release_id uuid NULL REFERENCES releases (id) ON DELETE CASCADE,
    artist_id uuid NULL REFERENCES artists (id) ON DELETE CASCADE,
    observed_artist_id uuid NULL REFERENCES observed_artists (id) ON DELETE CASCADE,
    normalized_name text NULL,
    external_provider text NULL,
    external_id text NULL,
    created_at timestamptz NOT NULL,

    CONSTRAINT catalog_identity_reconciliation_seed_target_check CHECK (
        release_id IS NOT NULL
        OR artist_id IS NOT NULL
        OR observed_artist_id IS NOT NULL
        OR btrim(COALESCE(normalized_name, '')) <> ''
        OR (external_provider IS NOT NULL AND external_id IS NOT NULL)
    )
);

-- The drain reads in id order and deletes a captured watermark, so this is the
-- only required access path. Existing tables carry the lookup indexes.
CREATE INDEX catalog_identity_reconciliation_seed_created_idx
    ON catalog_identity_reconciliation_seed (created_at, id);

-- PERF-014 / RM-18: acquire the Artist identity-name advisory locks in
-- ascending *computed lock-key* order in one call. Lexical name order does not
-- guarantee lock-key order, and a caller that used input order could deadlock
-- against another. This also replaces one round trip per name with one call.
CREATE OR REPLACE FUNCTION lock_artist_identity_names(p_names text[])
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    identity_name text;
BEGIN
    IF p_names IS NULL OR array_length(p_names, 1) IS NULL THEN
        RETURN;
    END IF;
    FOR identity_name IN
        SELECT name
        FROM unnest(p_names) AS name
        ORDER BY hashtextextended(name, 0), name
    LOOP
        PERFORM pg_advisory_xact_lock(hashtextextended(identity_name, 0));
    END LOOP;
END;
$$;
