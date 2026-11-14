-- ADR-0190: an imported Artist is `active`, not `provisional`.
--
-- The state itself is unchanged: an Artist created from observed credit text is
-- live, usable, and not yet identity-verified. Only its name changes, because
-- "provisional" described the absence of an operator assertion rather than what
-- the row is. Existing rows are relabelled in place; public IDs, tombstones,
-- decisions, evidence, and audit history are untouched.
--
-- `audio_identities` keeps its own `provisional` vocabulary: that state is not an
-- Artist state and is not operator-facing.
ALTER TABLE artists DROP CONSTRAINT IF EXISTS artists_status_check;

UPDATE artists SET status = 'active' WHERE status = 'provisional';

ALTER TABLE artists
    ADD CONSTRAINT artists_status_check
    CHECK (status IN ('active', 'verified', 'merged', 'deprecated'));
