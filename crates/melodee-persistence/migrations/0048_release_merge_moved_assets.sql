-- Multi-source Release merge asset provenance (ADR-0198).
--
-- `release_merge_events.moved_tracks` already records the immutable Track
-- ordinal map used by the reversible split. An administrative Release merge
-- also folds the source Release's artwork into the surviving Release and
-- detaches the source's Release-level billing rows from the observations that
-- move with its Tracks. Recording exactly what moved lets
-- `split_release_merge` restore those rows instead of guessing.
--
-- Shape (serialized `ReleaseMovedAssets`):
--   {
--     "detachedReleaseArtistObservations": [{"releaseArtistId","observedArtistId"}],
--     "adoptedOperatorImage": {"sourceReleaseId","updatedAt"} | null,
--     "adoptedProviderImages": ["<provider image id>"]
--   }
--
-- Release merge moves Tracks between Releases, but novelty evidence is
-- immutable policy history (ADR-0120): `track_novelty` rows must never be
-- rewritten or deleted, and `release_novelty_evaluations` stays unique per
-- evaluated Release. The original composite reference
-- (track_id, release_id, audio_identity_id, media_asset_id) therefore made a
-- Release that had completed policy evaluation impossible to merge, because
-- the Track's release coordinate may not change while its evaluation cannot.
--
-- The evidence keeps its evaluated Release; only the Track reference is
-- narrowed, so a merged Track no longer has to belong to the Release whose
-- evaluation recorded it. Nothing is rewritten: a split restores the original
-- coordinates exactly because the rows never moved.
ALTER TABLE track_novelty DROP CONSTRAINT track_novelty_exact_track_fk;
ALTER TABLE track_novelty
    ADD CONSTRAINT track_novelty_exact_track_fk FOREIGN KEY (track_id)
        REFERENCES tracks (id) ON DELETE RESTRICT;

-- Additive and safe for binary downgrade: existing merge events keep the
-- default empty object and split behaves exactly as before. Forward rollback
-- (ADR-0198):
--   ALTER TABLE track_novelty DROP CONSTRAINT track_novelty_exact_track_fk;
--   ALTER TABLE track_novelty
--       ADD CONSTRAINT track_novelty_exact_track_fk FOREIGN KEY (
--           track_id, release_id, audio_identity_id, media_asset_id
--       ) REFERENCES tracks (id, release_id, audio_identity_id, playable_media_asset_id);
--   (the composite constraint can only be restored once no merged Release has
--   novelty evidence recorded for a Track that now lives in another Release)
--   ALTER TABLE release_merge_events
--       DROP CONSTRAINT release_merge_events_moved_assets_object_check,
--       DROP COLUMN moved_assets;
-- Dropping the column discards only the asset provenance of merges performed
-- after this migration; it never touches Tracks, images, or media assets.
-- Restoring the immediate constraint requires no data change.

ALTER TABLE release_merge_events
    ADD COLUMN moved_assets jsonb NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE release_merge_events
    ADD CONSTRAINT release_merge_events_moved_assets_object_check
    CHECK (jsonb_typeof(moved_assets) = 'object');
