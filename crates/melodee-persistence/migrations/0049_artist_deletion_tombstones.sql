-- Artist deletion tombstones (ADR-0199).
--
-- Artists are the most connected catalog entities: identities, credits,
-- contributions, observations, provider evidence, user state, and audit
-- history all reference them. Physical row deletion would destroy that
-- graph, so an administrator deletion is a durable tombstone exactly like a
-- merged Artist or a deleted Release (ADR-0152).
--
-- `deleted` is a terminal state that the browse, resolver, provider, and
-- identity surfaces already exclude by filtering `status IN ('active',
-- 'verified')`. The merge check is unchanged: a deleted Artist is not merged
-- and therefore keeps `merged_into_artist_id` NULL.
--
-- Additive and reversible: existing rows are untouched. Rollback (ADR-0199):
--   UPDATE artists SET status = 'active' WHERE status = 'deleted';
--   ALTER TABLE artists DROP CONSTRAINT artists_status_check;
--   ALTER TABLE artists
--       ADD CONSTRAINT artists_status_check
--       CHECK (status IN ('active', 'verified', 'merged', 'deprecated'));
-- Rolling back un-deletes every deleted Artist, which is the operator-visible
-- intent of a rollback; the `artist_deleted` audit rows remain as evidence.

ALTER TABLE artists DROP CONSTRAINT IF EXISTS artists_status_check;

ALTER TABLE artists
    ADD CONSTRAINT artists_status_check
    CHECK (status IN ('active', 'verified', 'merged', 'deprecated', 'deleted'));

-- Deleted Artists are excluded from every browse/resolve predicate that
-- filters active/verified, so the admin list and the resolver stay fast.
CREATE INDEX IF NOT EXISTS artists_deleted_lookup_idx
    ON artists (id) WHERE status = 'deleted';
