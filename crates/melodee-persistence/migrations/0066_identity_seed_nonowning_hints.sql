-- Reconciliation seeds are durable work, not ownership of catalog identities.
-- Release mutations may already hold Track locks when they enqueue Artist and
-- Observation hints. FK checks would acquire parent KEY SHARE locks in the
-- reverse order to an Artist correction. Keep the UUIDs as non-owning hints;
-- candidate discovery resolves them with joins and safely ignores stale hints.
-- Names and external keys captured by deletion paths survive identity removal.
-- Applied with writers stopped under ADR-0166, like the other remediation DDL.
ALTER TABLE catalog_identity_reconciliation_seed
    DROP CONSTRAINT catalog_identity_reconciliation_seed_artist_id_fkey,
    DROP CONSTRAINT catalog_identity_reconciliation_seed_observed_artist_id_fkey;
