-- Indexes for the admin duplicate-candidate scans and Release merge history.
--
-- These are query-plan supports for read-only candidate discovery and for
-- reading the reversible Release merge/split history. They change no data,
-- no identity predicate, and no compatibility surface.

-- Provider candidate evidence is compared across Artists by provider/external
-- id. The existing index is Artist-first; the pair scan is external-first.
CREATE INDEX IF NOT EXISTS provider_identity_candidates_external_idx
    ON provider_identity_candidates (provider, external_id, artist_id);

-- Release duplicate candidates compare active Releases by normalized title
-- and display artist, then by title alone.
CREATE INDEX IF NOT EXISTS releases_active_title_artist_idx
    ON releases (normalized_title, lower(btrim(coalesce(display_artist, ''))), id)
    WHERE status = 'active';

-- Shared audio identity is the strongest non-provider Release duplicate
-- signal; the scan joins Tracks by audio identity and Release id.
CREATE INDEX IF NOT EXISTS tracks_audio_identity_release_idx
    ON tracks (audio_identity_id, release_id);

-- Release detail exposes its merge history so an operator or agent can call
-- the reversible split endpoint with the exact merge event id.
CREATE INDEX IF NOT EXISTS release_merge_events_source_idx
    ON release_merge_events (source_release_id, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS release_merge_events_target_idx
    ON release_merge_events (target_release_id, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS release_split_events_merge_idx
    ON release_split_events (merge_event_id, created_at DESC, id DESC);
