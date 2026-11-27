-- Jellyfin's Audio listing orders by track_number ASC NULLS LAST, then
-- public_api_id ASC, before applying LIMIT/OFFSET. The default ascending
-- B-tree null ordering matches that contract, including tied track numbers.
-- This additive index is built transactionally under the ADR-0166 maintenance
-- fence. It adds storage and write work for track inserts and updates to the
-- indexed keys; see the representative plan evidence for the measured tradeoff.
-- Roll back with a verified pre-migration restore or a forward migration that
-- drops this index under the same fence.
CREATE INDEX tracks_jellyfin_number_public_idx
    ON tracks (track_number, public_api_id);
